//! Resilient parser (SPEC-001): tokens → AST + diagnostics.
//!
//! - Recursive descent following the grammar in `docs/language.md`.
//! - Recovery: on error, emits `E005` (or `E006` with the Nalu form), skips to the end of the line or to the
//!   `}` that closes the block, and continues. Every loop consumes at least one token per iteration; nesting
//!   depth is limited (no stack overflow on `((((((…`).
//! - `{expr}` inside strings and commands is re-parsed here, with the offsets of the original file.

use crate::ast::*;
use crate::diag::{Diag, Span};
use crate::lexer::{self, CmdTok, Kw, LStrPart, LWord, StrTok, Tok, Token, WPart};

const MAX_DEPTH: usize = 64;

/// Parses a whole file.
pub fn parse(src: &str) -> (File, Vec<Diag>) {
    let (toks, mut diags) = lexer::lex(src, 0);
    let mut p = Parser::new(src, toks);
    let file = p.file();
    diags.append(&mut p.diags);
    (file, diags)
}

/// Parses a standalone expression (a string's `{expr}`, stdlib patterns). `base` is the offset in the file;
/// `full_src` is the whole file (for `assert` snippets).
pub fn parse_expr_at(full_src: &str, code: &str, base: usize, depth: usize) -> (Expr, Vec<Diag>) {
    let (toks, mut diags) = lexer::lex(code, base);
    let mut p = Parser::new(full_src, toks);
    p.depth = depth;
    let start = p.span();
    let e = match p.expr() {
        Ok(e) => {
            if !p.at_end_of_snippet() {
                let t = p.peek().clone();
                p.error(p.span(), format!("expected `}}` after the expression, found {}", t.describe()));
            }
            e
        }
        Err(()) => Expr { kind: ExprKind::Error, span: start },
    };
    diags.append(&mut p.diags);
    (e, diags)
}

type PResult<T> = Result<T, ()>;

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
    diags: Vec<Diag>,
    depth: usize,
    no_struct: bool,
    /// Parsing the `if` of a `match` arm: at its top level, `name =>` ends the guard (it isn't a lambda).
    in_guard: bool,
    /// A `param` was already parsed (`use` must come before it).
    seen_param: bool,
    last_err_pos: Option<usize>,
    /// Repeated errors at the same position (discarded).
    suppressed: Vec<Diag>,
}

fn is_assign_op(t: &Tok) -> Option<AssignOp> {
    Some(match t {
        Tok::Assign => AssignOp::Set,
        Tok::PlusEq => AssignOp::Add,
        Tok::MinusEq => AssignOp::Sub,
        Tok::StarEq => AssignOp::Mul,
        Tok::SlashEq => AssignOp::Div,
        Tok::PercentEq => AssignOp::Mod,
        _ => return None,
    })
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, toks: Vec<Token>) -> Parser<'a> {
        Parser { src, toks, pos: 0, diags: Vec::new(), depth: 0, no_struct: false, in_guard: false, seen_param: false, last_err_pos: None, suppressed: Vec::new() }
    }

    // ---------------------------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------------------------

    fn peek(&self) -> &Tok {
        &self.toks[self.pos.min(self.toks.len() - 1)].tok
    }
    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }
    fn span(&self) -> Span {
        self.toks[self.pos.min(self.toks.len() - 1)].span
    }
    fn peek_span_at(&self, n: usize) -> Span {
        self.toks[(self.pos + n).min(self.toks.len() - 1)].span
    }
    fn prev_span(&self) -> Span {
        if self.pos == 0 {
            self.span()
        } else {
            self.toks[(self.pos - 1).min(self.toks.len() - 1)].span
        }
    }
    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos.min(self.toks.len() - 1)].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn at(&self, t: &Tok) -> bool {
        self.peek() == t
    }
    fn at_kw(&self, k: Kw) -> bool {
        *self.peek() == Tok::Kw(k)
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn skip_nl(&mut self) {
        while matches!(self.peek(), Tok::Nl | Tok::Semi) {
            if *self.peek() == Tok::Semi {
                let sp = self.span();
                self.error6(sp, "`;` does not exist in Nalu", "one statement per line; remove the `;`");
            }
            self.bump();
        }
    }
    fn skip_nl_only(&mut self) {
        while *self.peek() == Tok::Nl {
            self.bump();
        }
    }
    fn at_end_of_snippet(&self) -> bool {
        matches!(self.peek(), Tok::Nl | Tok::Eof)
    }

    fn error(&mut self, span: Span, msg: impl Into<String>) -> &mut Diag {
        self.push_diag(Diag::error("E005", span, msg))
    }
    fn error6(&mut self, span: Span, msg: impl Into<String>, help: impl Into<String>) {
        self.push_diag(Diag::error("E006", span, msg).with_help(help));
    }
    fn push_diag(&mut self, d: Diag) -> &mut Diag {
        // at most one error per token position
        if self.last_err_pos == Some(d.span.start) {
            self.suppressed.push(d);
            return self.suppressed.last_mut().unwrap();
        }
        self.last_err_pos = Some(d.span.start);
        self.diags.push(d);
        self.diags.last_mut().unwrap()
    }

    fn unexpected(&mut self, what: &str) -> PResult<()> {
        let t = self.peek().clone();
        let sp = self.span();
        let found = t.describe();
        let d = self.error(sp, format!("expected {}, found {}", what, found));
        if t == Tok::Kw(Kw::Else) {
            d.help = Some("`else` must be on the same line as the `}` that closes the `if`, or right below it".into());
        }
        Err(())
    }

    fn expect(&mut self, t: Tok, what: &str) -> PResult<Span> {
        if self.at(&t) {
            Ok(self.bump().span)
        } else {
            self.unexpected(what)?;
            Err(())
        }
    }

    fn ident(&mut self, what: &str) -> PResult<Ident> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                let sp = self.bump().span;
                Ok(Ident { name, span: sp })
            }
            Tok::Kw(k) => {
                let sp = self.span();
                self.error(sp, format!("`{}` is a keyword and cannot be used as a name", k.text()))
                    .help = Some(format!("choose another name, for example `{}_`", k.text()));
                Err(())
            }
            _ => {
                self.unexpected(what)?;
                Err(())
            }
        }
    }

    /// Skips to the end of the line (consuming the `NL`) or to the `}` that closes the current block (without consuming it).
    fn sync(&mut self) {
        let mut depth: i32 = 0;
        loop {
            match self.peek() {
                Tok::Eof => return,
                Tok::Nl if depth <= 0 => {
                    self.bump();
                    return;
                }
                Tok::RBrace if depth <= 0 => return,
                Tok::LBrace | Tok::LParen | Tok::LBracket => depth += 1,
                Tok::RBrace | Tok::RParen | Tok::RBracket => depth -= 1,
                _ => {}
            }
            self.bump();
        }
    }

    fn enter(&mut self) -> PResult<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            let sp = self.span();
            self.error(sp, "nesting too deep");
            self.depth -= 1;
            return Err(());
        }
        Ok(())
    }
    fn leave(&mut self) {
        self.depth -= 1;
    }

    /// End of statement: `NL`, `}` (not consumed) or end of file.
    fn end_stmt(&mut self) -> PResult<()> {
        match self.peek() {
            Tok::Nl => {
                self.bump();
                Ok(())
            }
            Tok::Semi => {
                self.skip_nl();
                Ok(())
            }
            Tok::RBrace | Tok::Eof => Ok(()),
            Tok::PlusPlus | Tok::MinusMinus => {
                let sp = self.span();
                let op = if *self.peek() == Tok::PlusPlus { "+=" } else { "-=" };
                self.error6(sp, format!("`{}` does not exist", self.peek().punct_text()), format!("use `x {} 1`", op));
                Err(())
            }
            Tok::Question => {
                let sp = self.span();
                self.error6(sp, "the ternary `? :` does not exist", "use `if cond { a } else { b }` as an expression");
                Err(())
            }
            Tok::QQ => {
                let sp = self.span();
                self.error6(sp, "`??` does not exist", "for a default value use `catch`: `env.get(\"X\") catch \"default\"`");
                Err(())
            }
            Tok::FatArrow => {
                let sp = self.span();
                self.error6(sp, "`=>` out of place", "`=>` only appears in `match` arms (`pattern => value`) and in lambdas (`x => x * 2`)");
                Err(())
            }
            _ => {
                self.unexpected("end of line (one statement per line)")?;
                Err(())
            }
        }
    }

    // ---------------------------------------------------------------------------------------------
    // File and items
    // ---------------------------------------------------------------------------------------------

    fn file(&mut self) -> File {
        let mut items = Vec::new();
        let mut seen_other = false;
        self.skip_nl();
        while *self.peek() != Tok::Eof {
            let start = self.pos;
            let r = self.item(&mut seen_other);
            match r {
                Ok(item) => {
                    items.push(item);
                    if self.end_stmt().is_err() {
                        self.sync();
                    }
                }
                Err(()) => self.sync(),
            }
            if *self.peek() == Tok::RBrace {
                let sp = self.span();
                self.error(sp, "`}` without a matching `{`");
                self.bump();
            }
            if self.pos == start {
                self.bump();
            }
            self.skip_nl();
        }
        File { items }
    }

    fn item(&mut self, seen_other: &mut bool) -> PResult<Item> {
        match self.peek().clone() {
            Tok::Ident(n) if n == "use" && matches!(self.peek_at(1), Tok::Str(_)) => {
                let start = self.bump().span;
                let Tok::Str(s) = self.peek().clone() else { unreachable!("checked above") };
                let end = self.bump().span;
                let Some(path) = s.plain() else {
                    self.error(end, "the path of `use` is fixed text, without `{ }`");
                    return Err(());
                };
                if *seen_other || self.seen_param {
                    self.diags.push(Diag::error("E005", start.to(end), "`use` lines go at the start of the file").with_help("move them before the `param` lines and any other code"));
                }
                Ok(Item::Use(path, start.to(end)))
            }
            Tok::Kw(Kw::Param) => {
                self.seen_param = true;
                let p = self.param_decl()?;
                if *seen_other {
                    self.diags.push(
                        Diag::error("E005", p.span, "`param` can only appear at the start of the file")
                            .with_help("move the `param` lines before any functions, types and statements"),
                    );
                }
                Ok(Item::Param(p))
            }
            Tok::Kw(Kw::Fn) => {
                *seen_other = true;
                Ok(Item::Fn(self.fn_decl()?))
            }
            Tok::Kw(Kw::Struct) => {
                *seen_other = true;
                Ok(Item::Struct(self.struct_decl()?))
            }
            Tok::Kw(Kw::Enum) => {
                *seen_other = true;
                Ok(Item::Enum(self.enum_decl()?))
            }
            Tok::Kw(Kw::Test) => {
                *seen_other = true;
                Ok(Item::Test(self.test_decl()?))
            }
            _ => {
                *seen_other = true;
                Ok(Item::Stmt(self.stmt()?))
            }
        }
    }

    fn doc_here(&self) -> String {
        self.toks[self.pos.min(self.toks.len() - 1)].doc.clone().unwrap_or_default()
    }

    fn param_decl(&mut self) -> PResult<ParamDecl> {
        let doc = self.doc_here();
        let start = self.bump().span;
        let name = self.ident("a parameter name")?;
        self.expect(Tok::Colon, "`:` and the parameter type (`param name: str`)")?;
        let ty = self.ty()?;
        let default = if self.eat(&Tok::Assign) { Some(self.expr()?) } else { None };
        Ok(ParamDecl { name, ty, default, doc, span: start.to(self.prev_span()) })
    }

    fn fn_decl(&mut self) -> PResult<FnDecl> {
        let doc = self.doc_here();
        let start = self.bump().span;
        let name = self.ident("a function name")?;
        self.expect(Tok::LParen, "`(` with the parameters")?;
        let mut params = Vec::new();
        self.skip_nl_only();
        while !self.at(&Tok::RParen) {
            let pname = self.ident("a parameter name")?;
            self.expect(Tok::Colon, "`:` and the parameter type")?;
            let ty = self.ty()?;
            let default = if self.eat(&Tok::Assign) { Some(self.expr()?) } else { None };
            params.push(FnParam { name: pname, ty, default });
            self.skip_nl_only();
            if !self.eat(&Tok::Comma) {
                break;
            }
            self.skip_nl_only();
        }
        self.skip_nl_only();
        self.expect(Tok::RParen, "`)` after the parameters")?;
        let ret = if self.eat(&Tok::Arrow) {
            Some(self.ty()?)
        } else if *self.peek() == Tok::Colon {
            let sp = self.span();
            self.error6(sp, "the return type comes after `->`", "write `fn name() -> int {`");
            self.bump();
            Some(self.ty()?)
        } else {
            None
        };
        if let Tok::Ident(w) = self.peek() {
            if w == "throws" {
                let sp = self.span();
                self.error6(sp, "`throws` does not exist", "whether a function can fail is inferred; see `nalu check --explain`");
                self.bump();
            }
        }
        let body = self.block()?;
        Ok(FnDecl { name, params, ret, body, doc, span: start.to(self.prev_span()) })
    }

    fn struct_decl(&mut self) -> PResult<StructDecl> {
        let start = self.bump().span;
        let name = self.ident("a struct name")?;
        self.expect(Tok::LBrace, "`{` with the fields")?;
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        self.skip_nl();
        while !matches!(self.peek(), Tok::RBrace | Tok::Eof) {
            let before = self.pos;
            let r: PResult<()> = (|| {
                if self.at_kw(Kw::Fn) {
                    methods.push(self.fn_decl()?);
                } else {
                    let fname = self.ident("a field name or `fn`")?;
                    self.expect(Tok::Colon, "`:` and the field type")?;
                    let ty = self.ty()?;
                    let default = if self.eat(&Tok::Assign) { Some(self.expr()?) } else { None };
                    fields.push(FieldDecl { name: fname, ty, default });
                    self.eat(&Tok::Comma);
                }
                self.end_stmt()
            })();
            if r.is_err() {
                self.sync();
            }
            if self.pos == before {
                self.bump();
            }
            self.skip_nl();
        }
        self.expect(Tok::RBrace, "`}` at the end of the struct")?;
        Ok(StructDecl { name, fields, methods, span: start.to(self.prev_span()) })
    }

    fn enum_decl(&mut self) -> PResult<EnumDecl> {
        let start = self.bump().span;
        let name = self.ident("an enum name")?;
        self.expect(Tok::LBrace, "`{` with the cases")?;
        let mut cases = Vec::new();
        self.skip_nl();
        while !matches!(self.peek(), Tok::RBrace | Tok::Eof) {
            cases.push(self.ident("a case name")?);
            self.skip_nl();
            if !self.eat(&Tok::Comma) {
                self.skip_nl();
                if !self.at(&Tok::RBrace) && !matches!(self.peek(), Tok::Ident(_)) {
                    break;
                }
            }
            self.skip_nl();
        }
        self.expect(Tok::RBrace, "`}` at the end of the enum")?;
        Ok(EnumDecl { name, cases, span: start.to(self.prev_span()) })
    }

    fn test_decl(&mut self) -> PResult<TestDecl> {
        let start = self.bump().span;
        let name = match self.peek().clone() {
            Tok::Str(s) => {
                let sp = self.bump().span;
                match s.plain() {
                    Some(t) => t,
                    None => {
                        self.error(sp, "the test name is fixed text, without `{ }`");
                        String::new()
                    }
                }
            }
            _ => {
                self.unexpected("the test name in quotes (`test \"name\" {`)")?;
                return Err(());
            }
        };
        let body = self.block()?;
        Ok(TestDecl { name, body, span: start.to(self.prev_span()) })
    }

    /// Types as in Go: `[]T` (list) and `map[K]V` (map). The old syntax (`T[]`, `map<K, V>`) gets a hint.
    fn ty(&mut self) -> PResult<TypeExpr> {
        let t = match self.peek().clone() {
            Tok::LBracket if *self.peek_at(1) == Tok::RBracket => {
                let start = self.bump().span;
                self.bump();
                let inner = self.ty()?;
                let sp = start.to(inner.span());
                TypeExpr::List(Box::new(inner), sp)
            }
            Tok::Ident(n) if n == "map" && *self.peek_at(1) == Tok::LBracket => {
                let start = self.bump().span;
                self.bump();
                let k = self.ty()?;
                self.expect(Tok::RBracket, "`]` after the key type (`map[str]int`)")?;
                let v = self.ty()?;
                let sp = start.to(v.span());
                TypeExpr::Map(Box::new(k), Box::new(v), sp)
            }
            Tok::Ident(n) if n == "map" && *self.peek_at(1) == Tok::Lt => {
                let sp = self.span();
                self.error6(sp, "map types are written `map[K]V`", "for example `map[str]int`");
                return Err(());
            }
            Tok::Ident(_) => TypeExpr::Named(self.ident("a type")?),
            _ => {
                self.unexpected("a type (`str`, `int`, `bool`, `path`, `duration`, `[]str`, `map[str]int`…)")?;
                return Err(());
            }
        };
        if self.at(&Tok::LBracket) && *self.peek_at(1) == Tok::RBracket {
            let sp = t.span().to(self.peek_span_at(1));
            let name = match &t {
                TypeExpr::Named(i) => i.name.clone(),
                _ => "T".into(),
            };
            self.error6(sp, "list types are written `[]T`", format!("write `[]{}`", name));
            return Err(());
        }
        Ok(t)
    }

    // ---------------------------------------------------------------------------------------------
    // Blocks and statements
    // ---------------------------------------------------------------------------------------------

    fn block(&mut self) -> PResult<Block> {
        let open = self.expect(Tok::LBrace, "`{`")?;
        self.enter()?;
        let saved = self.no_struct;
        self.no_struct = false;
        let mut stmts = Vec::new();
        self.skip_nl();
        while !matches!(self.peek(), Tok::RBrace | Tok::Eof) {
            let before = self.pos;
            match self.stmt() {
                Ok(s) => {
                    stmts.push(s);
                    if self.end_stmt().is_err() {
                        self.sync();
                    }
                }
                Err(()) => self.sync(),
            }
            if self.pos == before {
                self.bump();
            }
            self.skip_nl();
        }
        self.no_struct = saved;
        self.leave();
        if self.at(&Tok::Eof) {
            self.error(open, "`{` without the `}` that closes the block").help = Some("check that every brace is closed".into());
            return Err(());
        }
        let close = self.bump().span;
        Ok(Block { stmts, span: open.to(close) })
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let kind = match self.peek().clone() {
            Tok::Kw(Kw::Let) | Tok::Kw(Kw::Var) => {
                let mutable = self.at_kw(Kw::Var);
                self.bump();
                let name = self.ident("a variable name")?;
                let ty = if self.eat(&Tok::Colon) { Some(self.ty()?) } else { None };
                if !self.at(&Tok::Assign) {
                    let sp = self.span();
                    self.error(sp, "every variable starts with a value: expected `=`").help =
                        Some(format!("write `{} {} = …`", if mutable { "var" } else { "let" }, name.name));
                    return Err(());
                }
                self.bump();
                let value = self.expr()?;
                StmtKind::Let { name, ty, value, mutable }
            }
            Tok::Kw(Kw::Return) => {
                self.bump();
                if matches!(self.peek(), Tok::Nl | Tok::RBrace | Tok::Eof) {
                    StmtKind::Return(None)
                } else {
                    StmtKind::Return(Some(self.expr()?))
                }
            }
            Tok::Kw(Kw::Break) => {
                self.bump();
                StmtKind::Break
            }
            Tok::Kw(Kw::Continue) => {
                self.bump();
                StmtKind::Continue
            }
            Tok::Kw(Kw::Fail) => {
                self.bump();
                let (message, code) = self.fail_args()?;
                StmtKind::Fail { message, code }
            }
            Tok::Kw(Kw::For) => self.for_stmt()?,
            Tok::Kw(Kw::While) => {
                self.bump();
                let cond = self.cond()?;
                let body = self.block()?;
                StmtKind::While { cond, body }
            }
            Tok::Kw(Kw::Defer) => {
                self.bump();
                StmtKind::Defer(self.expr()?)
            }
            Tok::Kw(Kw::Native) => self.native()?,
            Tok::Kw(k @ (Kw::Fn | Kw::Struct | Kw::Enum | Kw::Param | Kw::Test)) => {
                let sp = self.span();
                self.error(sp, format!("`{}` can only appear at the top level of the file", k.text()));
                let mut dummy = true;
                let _ = self.item(&mut dummy);
                return Err(());
            }
            Tok::Kw(Kw::Else) => {
                let sp = self.span();
                self.error(sp, "`else` without `if`").help = Some("`else` comes right after the `}` that closes the `if`: `} else {`".into());
                return Err(());
            }
            Tok::Ident(ref w) if w == "exit" && *self.peek_at(1) == Tok::LParen => {
                self.bump();
                self.bump();
                self.skip_nl_only();
                let code = if self.at(&Tok::RParen) { None } else { Some(self.expr()?) };
                self.skip_nl_only();
                self.expect(Tok::RParen, "`)`")?;
                StmtKind::Exit(code)
            }
            Tok::Ident(ref w) if w == "assert" && !matches!(self.peek_at(1), Tok::Assign | Tok::Dot | Tok::Nl) => {
                self.bump();
                let cs = self.span();
                let cond = self.expr()?;
                let text = self.text_of(cs.to(cond.span));
                StmtKind::Assert { cond, text }
            }
            Tok::Ident(ref w) if w == "assert_fails" && *self.peek_at(1) == Tok::LParen => {
                self.bump();
                self.bump();
                let e = self.expr()?;
                self.expect(Tok::RParen, "`)`")?;
                StmtKind::AssertFails(e)
            }
            Tok::Ident(ref w) if self.e006_keyword(w) => return Err(()),
            Tok::Ident(ref w) if w == "print" && matches!(self.peek_at(1), Tok::Str(_) | Tok::Ident(_) | Tok::Int(_)) => {
                let sp = self.span();
                self.error6(sp, "`print` is a function: the arguments go in parentheses", "write `print(\"text\")`");
                return Err(());
            }
            _ => {
                let e = self.expr()?;
                if let Some(op) = is_assign_op(self.peek()) {
                    self.bump();
                    let value = self.expr()?;
                    StmtKind::Assign { target: e, op, value }
                } else {
                    StmtKind::Expr(e)
                }
            }
        };
        Ok(Stmt { kind, span: start.to(self.prev_span()) })
    }

    fn text_of(&self, sp: Span) -> String {
        self.src.get(sp.start..sp.end).unwrap_or("").trim().to_string()
    }

    /// Words from other languages with the right Nalu form (`E006`). Returns `true` if it emitted the error.
    fn e006_keyword(&mut self, w: &str) -> bool {
        let next = self.peek_at(1).clone();
        let looks_like_use = matches!(next, Tok::Ident(_) | Tok::Str(_) | Tok::Int(_) | Tok::LBrace);
        let sp = self.span();
        let (msg, help): (String, &str) = match w {
            "const" if looks_like_use => ("`const` does not exist".into(), "use `let` (immutable) or `var` (mutable)"),
            "func" | "function" | "def" | "fun" if looks_like_use => (format!("`{}` does not exist", w), "functions are declared with `fn name(x: int) -> int { … }`"),
            "switch" if looks_like_use || next == Tok::LParen => ("`switch` does not exist".into(), "use `match value { pattern => … }`"),
            "case" if looks_like_use => ("`case` does not exist".into(), "inside `match`, each arm is `pattern => …`"),
            "throw" | "raise" if looks_like_use || next == Tok::LParen => (format!("`{}` does not exist", w), "use `fail \"message\"`"),
            "import" | "require" | "include" | "use" if looks_like_use => (
                format!("`{}` does not exist", w),
                "the standard library is always available (`fs.read`, `log.info`…); remove the line",
            ),
            "elif" | "elsif" if looks_like_use => (format!("`{}` does not exist", w), "use `} else if cond {`"),
            _ => return false,
        };
        self.error6(sp, msg, help);
        true
    }

    /// `fail "message"` or `fail "message", code: n` (a single form, like `return`). The form with parentheses
    /// and a comma (`fail("msg", code: n)`) gets a hint.
    fn fail_args(&mut self) -> PResult<(Expr, Option<Expr>)> {
        if self.at(&Tok::LParen) {
            let save = self.pos;
            let sp = self.span();
            self.bump();
            self.skip_nl_only();
            let saved_ns = self.no_struct;
            self.no_struct = false;
            let msg = self.expr();
            self.no_struct = saved_ns;
            if msg.is_ok() && self.at(&Tok::Comma) {
                self.error6(sp, "`fail` takes no parentheses", "write `fail \"message\", code: 3`");
                return Err(());
            }
            // it was an expression starting with parentheses: `fail ("a" + b)`
            self.pos = save;
        }
        let msg = self.expr()?;
        // `, code: n` (only with `code:` after the comma: `print(x catch fail "a", y)` stays an argument)
        if self.at(&Tok::Comma) && matches!(self.peek_at(1), Tok::Ident(n) if n == "code") && *self.peek_at(2) == Tok::Colon {
            self.bump();
            self.bump();
            self.bump();
            let code = self.expr()?;
            return Ok((msg, Some(code)));
        }
        Ok((msg, None))
    }

    fn for_stmt(&mut self) -> PResult<StmtKind> {
        self.bump();
        if self.at(&Tok::LParen) {
            let sp = self.span();
            self.error6(sp, "C-style loops do not exist", "use `for i in 0..10` or `while cond { … }`");
            return Err(());
        }
        let first = self.ident("a loop variable name")?;
        let second = if self.eat(&Tok::Comma) { Some(self.ident("the loop's second name")?) } else { None };
        match self.peek() {
            Tok::Kw(Kw::In) => {
                self.bump();
            }
            Tok::Ident(w) if w == "of" => {
                let sp = self.span();
                self.error6(sp, "`for … of` does not exist", "use `for x in list`");
                return Err(());
            }
            Tok::Assign | Tok::Colon => {
                let sp = self.span();
                self.error6(sp, "C-style loops do not exist", "use `for i in 0..10` or `while cond { … }`");
                return Err(());
            }
            _ => {
                self.unexpected("`in`")?;
            }
        }
        let iter = self.cond_expr()?;
        let body = self.block()?;
        Ok(StmtKind::For { first, second, iter, body })
    }

    fn native(&mut self) -> PResult<StmtKind> {
        let start = self.bump().span;
        self.expect(Tok::LBrace, "`{` with `bash:` and `powershell:`")?;
        let mut bash = None;
        let mut ps = None;
        self.skip_nl();
        while !matches!(self.peek(), Tok::RBrace | Tok::Eof) {
            let key = self.ident("`bash:` or `powershell:`")?;
            self.expect(Tok::Colon, "`:`")?;
            let text = match self.peek().clone() {
                Tok::Str(s) => {
                    let sp = self.bump().span;
                    match s.plain() {
                        Some(t) => t,
                        None => {
                            self.diags.push(
                                Diag::error("E052", sp, "`native` code is fixed text, without `{ }`").with_help("use single quotes: `'code'`"),
                            );
                            String::new()
                        }
                    }
                }
                _ => {
                    self.unexpected("the code in single quotes (`'…'`)")?;
                    return Err(());
                }
            };
            match key.name.as_str() {
                "bash" => bash = Some(text),
                "powershell" => ps = Some(text),
                other => {
                    self.diags.push(
                        Diag::error("E052", key.span, format!("`native` does not know `{}`", other)).with_help("the keys are `bash:` and `powershell:`"),
                    );
                }
            }
            self.eat(&Tok::Comma);
            self.skip_nl();
        }
        let end = self.expect(Tok::RBrace, "`}`")?;
        let _ = start.to(end);
        Ok(StmtKind::Native { bash, powershell: ps })
    }

    // ---------------------------------------------------------------------------------------------
    // Expressions
    // ---------------------------------------------------------------------------------------------

    /// `if`/`while` condition: no struct literals; `$ cmd` becomes a test.
    fn cond(&mut self) -> PResult<Expr> {
        let e = self.cond_expr()?;
        Ok(to_cmd_test(e))
    }

    fn cond_expr(&mut self) -> PResult<Expr> {
        let saved = self.no_struct;
        self.no_struct = true;
        let r = self.expr();
        self.no_struct = saved;
        r
    }

    pub fn expr(&mut self) -> PResult<Expr> {
        self.enter()?;
        let r = self.or_expr();
        self.leave();
        r
    }

    fn or_expr(&mut self) -> PResult<Expr> {
        let lhs = self.pipe_expr()?;
        // `a or b` / `a and b` from Python users: logical "or" is `||`, "and" is `&&`
        if let Tok::Ident(w) = self.peek().clone() {
            if w == "or" || w == "and" {
                let sp = self.span();
                let (op, other) = if w == "or" { ("||", "; to handle a failure use `catch`: `x catch default`") } else { ("&&", "") };
                self.error6(sp, format!("`{}` does not exist in Nalu", w), format!("the logical {} is `{}`{}", if w == "or" { "or" } else { "and" }, op, other));
                return Err(());
            }
        }
        if self.at_kw(Kw::Catch) {
            self.bump();
            let h = self.handler()?;
            let end = self.prev_span();
            let sp = lhs.span.to(end);
            return Ok(Expr { kind: ExprKind::Or(Box::new(lhs), Box::new(h)), span: sp });
        }
        Ok(lhs)
    }

    fn handler(&mut self) -> PResult<Handler> {
        self.enter()?;
        let r = (|| match self.peek().clone() {
            Tok::Kw(Kw::Ignore) => Ok(Handler::Ignore(self.bump().span)),
            Tok::Kw(Kw::Fail) => {
                let s = self.bump().span;
                let (message, code) = self.fail_args()?;
                Ok(Handler::Fail { message, code, span: s.to(self.prev_span()) })
            }
            Tok::LBrace => Ok(Handler::Block(self.block()?)),
            Tok::Kw(Kw::Return) | Tok::Kw(Kw::Break) | Tok::Kw(Kw::Continue) => {
                let sp = self.span();
                let w = self.peek().describe();
                self.error(sp, format!("`catch` must be followed by a value, `fail`, `ignore` or a block; found {}", w)).help =
                    Some("put the statement in a block: `catch { return }`".into());
                Err(())
            }
            Tok::Ident(w) if w == "exit" => {
                let sp = self.span();
                self.error(sp, "`exit` is a statement and cannot follow `catch` without a block").help = Some("write `catch { exit(1) }`".into());
                Err(())
            }
            _ => Ok(Handler::Value(self.or_expr()?)),
        })();
        self.leave();
        r
    }

    fn pipe_expr(&mut self) -> PResult<Expr> {
        let lhs = self.oror()?;
        if self.at(&Tok::Pipe) {
            if let Tok::Cmd(c) = self.peek_at(1).clone() {
                self.bump();
                let t = self.bump();
                let mut cmd = self.cmd_ast(&c, t.span);
                let sp = lhs.span.to(t.span);
                cmd.stdin = Some(lhs);
                return Ok(Expr { kind: ExprKind::Command(Box::new(cmd)), span: sp });
            }
            let sp = self.span();
            self.error(sp, "`|` outside a command").help =
                Some("to send a value to a command: `value | $ command`; logical \"or\" is `||`".into());
            return Err(());
        }
        Ok(lhs)
    }

    fn binary_left(&mut self, next: fn(&mut Self) -> PResult<Expr>, ops: &[(Tok, BinOp)]) -> PResult<Expr> {
        let mut lhs = next(self)?;
        loop {
            let op = ops.iter().find(|(t, _)| t == self.peek()).map(|(_, o)| *o);
            let Some(op) = op else { break };
            self.bump();
            self.skip_nl_only();
            let rhs = next(self)?;
            let sp = lhs.span.to(rhs.span);
            lhs = Expr { kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), span: sp };
        }
        Ok(lhs)
    }

    fn oror(&mut self) -> PResult<Expr> {
        self.binary_left(Self::andand, &[(Tok::OrOr, BinOp::Or)])
    }
    fn andand(&mut self) -> PResult<Expr> {
        self.binary_left(Self::cmp, &[(Tok::AndAnd, BinOp::And)])
    }
    fn cmp(&mut self) -> PResult<Expr> {
        let lhs = self.range()?;
        let op = match self.peek() {
            Tok::EqEq => BinOp::Eq,
            Tok::Ne => BinOp::Ne,
            Tok::Lt => BinOp::Lt,
            Tok::Le => BinOp::Le,
            Tok::Gt => BinOp::Gt,
            Tok::Ge => BinOp::Ge,
            Tok::Assign if self.no_struct => {
                let sp = self.span();
                self.error(sp, "`=` assigns; to compare use `==`");
                return Err(());
            }
            _ => return Ok(lhs),
        };
        self.bump();
        self.skip_nl_only();
        let rhs = self.range()?;
        if matches!(self.peek(), Tok::EqEq | Tok::Ne | Tok::Lt | Tok::Le | Tok::Gt | Tok::Ge) {
            let sp = self.span();
            self.error(sp, "comparisons cannot be chained").help = Some("join them with `&&`: `a < b && b < c`".into());
            return Err(());
        }
        let sp = lhs.span.to(rhs.span);
        Ok(Expr { kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), span: sp })
    }
    fn range(&mut self) -> PResult<Expr> {
        let lhs = self.sum()?;
        let inclusive = match self.peek() {
            Tok::DotDot => false,
            Tok::DotDotEq => true,
            _ => return Ok(lhs),
        };
        self.bump();
        let rhs = self.sum()?;
        let sp = lhs.span.to(rhs.span);
        Ok(Expr { kind: ExprKind::Range { from: Box::new(lhs), to: Box::new(rhs), inclusive }, span: sp })
    }
    fn sum(&mut self) -> PResult<Expr> {
        self.binary_left(Self::term, &[(Tok::Plus, BinOp::Add), (Tok::Minus, BinOp::Sub)])
    }
    fn term(&mut self) -> PResult<Expr> {
        self.binary_left(Self::unary, &[(Tok::Star, BinOp::Mul), (Tok::Slash, BinOp::Div), (Tok::Percent, BinOp::Mod)])
    }
    fn unary(&mut self) -> PResult<Expr> {
        match self.peek() {
            Tok::Minus | Tok::Bang => {
                let neg = *self.peek() == Tok::Minus;
                let s = self.bump().span;
                self.enter()?;
                let e = self.unary();
                self.leave();
                let e = e?;
                let sp = s.to(e.span);
                if neg {
                    match e.kind {
                        ExprKind::Int(n) => return Ok(Expr { kind: ExprKind::Int(n.wrapping_neg()), span: sp }),
                        ExprKind::Duration(n) => return Ok(Expr { kind: ExprKind::Duration(-n), span: sp }),
                        _ => {}
                    }
                }
                let op = if neg { UnOp::Neg } else { UnOp::Not };
                Ok(Expr { kind: ExprKind::Unary(op, Box::new(e)), span: sp })
            }
            Tok::PlusPlus | Tok::MinusMinus => {
                let sp = self.span();
                let op = if *self.peek() == Tok::PlusPlus { "+=" } else { "-=" };
                self.error6(sp, format!("`{}` does not exist", self.peek().punct_text()), format!("use `x {} 1`", op));
                Err(())
            }
            _ => self.postfix(),
        }
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        loop {
            match self.peek() {
                Tok::Dot => {
                    self.bump();
                    let name = match self.peek().clone() {
                        Tok::Ident(n) => Ident { name: n, span: self.bump().span },
                        Tok::Kw(k) => Ident { name: k.text().to_string(), span: self.bump().span },
                        Tok::Int(_) => {
                            let sp = self.span();
                            self.error(sp, "access by number with `.` does not exist").help = Some("use `list[0]`".into());
                            return Err(());
                        }
                        _ => {
                            self.unexpected("a name after `.`")?;
                            return Err(());
                        }
                    };
                    let sp = e.span.to(name.span);
                    e = Expr { kind: ExprKind::Field(Box::new(e), name), span: sp };
                }
                Tok::LParen => {
                    let (args, end) = self.call_args()?;
                    let sp = e.span.to(end);
                    e = Expr { kind: ExprKind::Call { callee: Box::new(e), args }, span: sp };
                }
                Tok::LBracket => {
                    self.bump();
                    let saved = self.no_struct;
                    self.no_struct = false;
                    self.skip_nl_only();
                    let idx = self.expr();
                    self.no_struct = saved;
                    let idx = idx?;
                    self.skip_nl_only();
                    let end = self.expect(Tok::RBracket, "`]`")?;
                    let sp = e.span.to(end);
                    e = Expr { kind: ExprKind::Index(Box::new(e), Box::new(idx)), span: sp };
                }
                Tok::Question => {
                    let sp = self.span();
                    self.error6(sp, "neither the ternary `? :` nor `?` exists", "use `if cond { a } else { b }`; for failures, `catch`");
                    return Err(());
                }
                Tok::QQ => {
                    let sp = self.span();
                    self.error6(sp, "`??` does not exist", "for a default value use `catch`: `m[\"k\"] catch 0`");
                    return Err(());
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn call_args(&mut self) -> PResult<(Vec<Arg>, Span)> {
        self.expect(Tok::LParen, "`(`")?;
        let saved = self.no_struct;
        self.no_struct = false;
        let r = (|| {
            let mut args = Vec::new();
            self.skip_nl_only();
            while !self.at(&Tok::RParen) {
                let name = if matches!(self.peek(), Tok::Ident(_)) && *self.peek_at(1) == Tok::Colon {
                    let n = self.ident("a name")?;
                    self.bump();
                    self.skip_nl_only();
                    Some(n)
                } else {
                    None
                };
                let value = self.expr()?;
                args.push(Arg { name, value });
                self.skip_nl_only();
                if !self.eat(&Tok::Comma) {
                    break;
                }
                self.skip_nl_only();
            }
            self.skip_nl_only();
            let end = self.expect(Tok::RParen, "`,` or `)` in the arguments")?;
            Ok((args, end))
        })();
        self.no_struct = saved;
        r
    }

    fn primary(&mut self) -> PResult<Expr> {
        let t = self.peek().clone();
        let sp = self.span();
        match t {
            Tok::Int(n) => {
                self.bump();
                Ok(Expr { kind: ExprKind::Int(n), span: sp })
            }
            Tok::Dur(n) => {
                self.bump();
                Ok(Expr { kind: ExprKind::Duration(n), span: sp })
            }
            Tok::Kw(Kw::True) => {
                self.bump();
                Ok(Expr { kind: ExprKind::Bool(true), span: sp })
            }
            Tok::Kw(Kw::False) => {
                self.bump();
                Ok(Expr { kind: ExprKind::Bool(false), span: sp })
            }
            Tok::Kw(Kw::SelfKw) => {
                self.bump();
                Ok(Expr { kind: ExprKind::SelfVal, span: sp })
            }
            Tok::Str(s) => {
                self.bump();
                Ok(self.str_expr(&s, sp))
            }
            Tok::Ident(name) => {
                if *self.peek_at(1) == Tok::FatArrow && !(self.in_guard && self.no_struct) {
                    if self.no_struct {
                        let a = self.toks[(self.pos + 1).min(self.toks.len() - 1)].span;
                        self.error6(a, "`=>` out of place", "`=>` only appears in `match` arms and in lambdas passed to `filter`, `map`…");
                        return Err(());
                    }
                    return self.lambda();
                }
                if name == "not" && matches!(self.peek_at(1), Tok::Ident(_) | Tok::LParen | Tok::Kw(_) | Tok::Int(_) | Tok::Str(_) | Tok::Bang) {
                    self.error6(sp, "`not` does not exist in Nalu", "the logical not is `!`: `!ok`");
                    return Err(());
                }
                if matches!(name.as_str(), "null" | "nil" | "None" | "undefined") {
                    self.error6(sp, format!("`{}` does not exist", name), "a missing value is a failure: handle it with `catch` (`env.get(\"X\") catch \"default\"`)");
                    return Err(());
                }
                self.bump();
                let starts_upper = name.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false);
                if starts_upper && !self.no_struct && self.at(&Tok::LBrace) {
                    return self.brace_literal(Some(Ident { name, span: sp }), sp);
                }
                Ok(Expr { kind: ExprKind::Ident(name), span: sp })
            }
            Tok::LParen => {
                if self.is_paren_lambda() && !(self.in_guard && self.no_struct) {
                    return self.lambda();
                }
                self.bump();
                let saved = self.no_struct;
                self.no_struct = false;
                self.skip_nl_only();
                let e = self.expr();
                self.no_struct = saved;
                let mut e = e?;
                self.skip_nl_only();
                let end = self.expect(Tok::RParen, "`)`")?;
                e.span = sp.to(end);
                Ok(e)
            }
            Tok::LBracket => self.list_literal(),
            Tok::LBrace => {
                // in a condition, `{` opens the block, except an unambiguous map: `{ "a": 1 }`
                let map_ahead = matches!(self.peek_at(1), Tok::Str(_) | Tok::Int(_)) && *self.peek_at(2) == Tok::Colon;
                if self.no_struct && !map_ahead {
                    self.unexpected("an expression before `{`")?;
                    return Err(());
                }
                self.brace_literal(None, sp)
            }
            Tok::Kw(Kw::If) => self.if_expr(),
            Tok::Kw(Kw::Match) => self.match_expr(),
            Tok::Kw(Kw::Try) => {
                self.bump();
                if !self.at(&Tok::LBrace) {
                    self.error6(sp, "`try` before an expression does not exist", "failures already propagate on their own; to handle the failure use `x catch …`, or `try { … } catch …` for a block");
                    return Err(());
                }
                let b = self.block()?;
                let s2 = sp.to(b.span);
                Ok(Expr { kind: ExprKind::Try(b), span: s2 })
            }
            Tok::Kw(Kw::Retry) => {
                self.bump();
                let (args, _) = if self.at(&Tok::LParen) {
                    self.call_args()?
                } else {
                    self.error(sp, "`retry` needs the number of attempts").help = Some("write `retry(5, every: 2s) { … }`".into());
                    return Err(());
                };
                let mut times = None;
                let mut every = None;
                for a in args {
                    match &a.name {
                        None if times.is_none() => times = Some(Box::new(a.value)),
                        Some(n) if n.name == "every" && every.is_none() => every = Some(Box::new(a.value)),
                        Some(n) => {
                            self.diags.push(
                                Diag::error("E012", n.span, format!("`retry` has no parameter `{}`", n.name)).with_help("the form is `retry(n, every: 2s)`"),
                            );
                        }
                        None => {
                            self.diags.push(Diag::error("E012", a.value.span, "extra argument to `retry`").with_help("the form is `retry(n, every: 2s)`"));
                        }
                    }
                }
                let times = match times {
                    Some(t) => t,
                    None => {
                        self.diags.push(Diag::error("E012", sp, "missing the number of attempts for `retry`").with_help("the form is `retry(n, every: 2s)`"));
                        Box::new(Expr { kind: ExprKind::Error, span: sp })
                    }
                };
                let body = self.block()?;
                let s2 = sp.to(body.span);
                Ok(Expr { kind: ExprKind::Retry { times, every, body }, span: s2 })
            }
            Tok::Kw(Kw::Parallel) => self.parallel(),
            Tok::Cmd(c) => {
                self.bump();
                let cmd = self.cmd_ast(&c, sp);
                Ok(Expr { kind: ExprKind::Command(Box::new(cmd)), span: sp })
            }
            Tok::Capture(c) => {
                self.bump();
                let cmd = self.cmd_ast(&c, sp);
                Ok(Expr { kind: ExprKind::Capture(Box::new(cmd)), span: sp })
            }
            Tok::Kw(Kw::Fail) => {
                self.error(sp, "`fail` is a statement and cannot appear in the middle of an expression").help =
                    Some("use `fail` on its own line, in a `match` arm or after `catch`".into());
                Err(())
            }
            Tok::PlusPlus | Tok::MinusMinus => {
                self.error6(sp, "`++`/`--` do not exist", "use `x += 1`");
                Err(())
            }
            Tok::FatArrow => {
                self.error6(sp, "`=>` out of place", "`=>` only appears in `match` arms and in lambdas (`x => x * 2`)");
                Err(())
            }
            _ => {
                self.unexpected("an expression")?;
                Err(())
            }
        }
    }

    fn is_paren_lambda(&self) -> bool {
        // ( a , b ) =>
        let mut i = 1;
        loop {
            match self.peek_at(i) {
                Tok::Ident(_) => {}
                Tok::RParen if i == 1 => return *self.peek_at(i + 1) == Tok::FatArrow,
                _ => return false,
            }
            i += 1;
            match self.peek_at(i) {
                Tok::Comma => i += 1,
                Tok::RParen => return *self.peek_at(i + 1) == Tok::FatArrow,
                _ => return false,
            }
        }
    }

    fn lambda(&mut self) -> PResult<Expr> {
        let start = self.span();
        let mut params = Vec::new();
        if self.eat(&Tok::LParen) {
            while !self.at(&Tok::RParen) {
                params.push(self.ident("a parameter name")?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(Tok::RParen, "`)`")?;
        } else {
            params.push(self.ident("a parameter name")?);
        }
        self.expect(Tok::FatArrow, "`=>`")?;
        let saved = self.no_struct;
        self.no_struct = false;
        let body = if self.at(&Tok::LBrace) {
            self.block().map(LambdaBody::Block)
        } else {
            self.expr().map(|e| LambdaBody::Expr(Box::new(e)))
        };
        self.no_struct = saved;
        let body = body?;
        Ok(Expr { kind: ExprKind::Lambda { params, body }, span: start.to(self.prev_span()) })
    }

    fn list_literal(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let saved = self.no_struct;
        self.no_struct = false;
        let r = (|| {
            let mut items = Vec::new();
            self.skip_nl_only();
            while !self.at(&Tok::RBracket) {
                items.push(self.expr()?);
                self.skip_nl_only();
                if !self.eat(&Tok::Comma) {
                    break;
                }
                self.skip_nl_only();
            }
            self.skip_nl_only();
            let end = self.expect(Tok::RBracket, "`,` or `]` in the list")?;
            Ok(Expr { kind: ExprKind::List(items), span: start.to(end) })
        })();
        self.no_struct = saved;
        r
    }

    /// `{ … }` in value position: map, unnamed struct or `{}`. With `name`, a named struct.
    fn brace_literal(&mut self, name: Option<Ident>, start: Span) -> PResult<Expr> {
        self.expect(Tok::LBrace, "`{`")?;
        let saved = self.no_struct;
        self.no_struct = false;
        let r = (|| {
            self.skip_nl_only();
            if self.at(&Tok::RBrace) && name.is_none() {
                let end = self.bump().span;
                return Ok(Expr { kind: ExprKind::EmptyBraces, span: start.to(end) });
            }
            let is_struct = name.is_some()
                || *self.peek() == Tok::DotDot
                || (matches!(self.peek(), Tok::Ident(_)) && *self.peek_at(1) == Tok::Colon);
            if is_struct {
                let mut spread = None;
                let mut fields = Vec::new();
                while !self.at(&Tok::RBrace) {
                    if self.eat(&Tok::DotDot) {
                        let e = self.expr()?;
                        if spread.is_some() {
                            self.error(e.span, "only one `..base` per literal");
                        }
                        spread = Some(Box::new(e));
                    } else {
                        let f = self.ident("a field name")?;
                        if !self.at(&Tok::Colon) {
                            let sp = self.span();
                            self.error(sp, "expected `:` and the field value").help = Some(format!("write `{}: value`", f.name));
                            return Err(());
                        }
                        self.bump();
                        self.skip_nl_only();
                        let v = self.expr()?;
                        fields.push((f, v));
                    }
                    self.skip_nl_only();
                    if !self.eat(&Tok::Comma) {
                        self.skip_nl_only();
                        if !matches!(self.peek(), Tok::Ident(_) | Tok::DotDot) {
                            break;
                        }
                    }
                    self.skip_nl_only();
                }
                self.skip_nl_only();
                let end = self.expect(Tok::RBrace, "`,` or `}` in the literal")?;
                return Ok(Expr { kind: ExprKind::StructLit { name: name.clone(), spread, fields }, span: start.to(end) });
            }
            let mut entries = Vec::new();
            while !self.at(&Tok::RBrace) {
                let k = self.expr()?;
                if !self.at(&Tok::Colon) {
                    let sp = self.span();
                    self.error(sp, "expected `:` between the map key and value");
                    return Err(());
                }
                self.bump();
                self.skip_nl_only();
                let v = self.expr()?;
                entries.push((k, v));
                self.skip_nl_only();
                if !self.eat(&Tok::Comma) {
                    break;
                }
                self.skip_nl_only();
            }
            self.skip_nl_only();
            let end = self.expect(Tok::RBrace, "`,` or `}` in the map")?;
            Ok(Expr { kind: ExprKind::Map(entries), span: start.to(end) })
        })();
        self.no_struct = saved;
        r
    }

    fn if_expr(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let cond = self.cond()?;
        let then = self.block()?;
        // `else` on the same line or the next
        let mut look = self.pos;
        while self.toks.get(look).map(|t| t.tok == Tok::Nl).unwrap_or(false) {
            look += 1;
        }
        if let Some(Tok::Ident(w)) = self.toks.get(look).map(|t| &t.tok) {
            if w == "elif" || w == "elsif" {
                let sp = self.toks[look].span;
                self.error6(sp, format!("`{}` does not exist", w), "use `} else if cond {`");
                return Err(());
            }
        }
        let els = if self.toks.get(look).map(|t| t.tok == Tok::Kw(Kw::Else)).unwrap_or(false) {
            self.pos = look;
            self.bump();
            if self.at_kw(Kw::If) {
                self.enter()?;
                let nested = self.if_expr();
                self.leave();
                let nested = nested?;
                let sp = nested.span;
                Some(Block { stmts: vec![Stmt { kind: StmtKind::Expr(nested), span: sp }], span: sp })
            } else {
                Some(self.block()?)
            }
        } else {
            None
        };
        let end = self.prev_span();
        Ok(Expr { kind: ExprKind::If { cond: Box::new(cond), then, els }, span: start.to(end) })
    }

    fn match_expr(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let subject = self.cond_expr()?;
        self.expect(Tok::LBrace, "`{` with the match arms")?;
        self.enter()?;
        let saved = self.no_struct;
        self.no_struct = false;
        let mut arms = Vec::new();
        self.skip_nl();
        while !matches!(self.peek(), Tok::RBrace | Tok::Eof) {
            let before = self.pos;
            match self.arm() {
                Ok(a) => {
                    arms.push(a);
                    match self.peek() {
                        Tok::Nl => {
                            self.bump();
                        }
                        Tok::RBrace | Tok::Eof => {}
                        Tok::Comma => {
                            let sp = self.span();
                            self.error(sp, "`match` arms do not take commas").help = Some("one arm per line".into());
                            self.sync();
                        }
                        _ => {
                            let _ = self.unexpected("end of line after the arm (one arm per line)");
                            self.sync();
                        }
                    }
                }
                Err(()) => self.sync(),
            }
            if self.pos == before {
                self.bump();
            }
            self.skip_nl();
        }
        self.no_struct = saved;
        self.leave();
        let end = self.expect(Tok::RBrace, "`}` at the end of the match")?;
        Ok(Expr { kind: ExprKind::Match { subject: Box::new(subject), arms }, span: start.to(end) })
    }

    fn arm(&mut self) -> PResult<MatchArm> {
        let start = self.span();
        let mut patterns = vec![self.pattern()?];
        while self.eat(&Tok::Pipe) {
            patterns.push(self.pattern()?);
        }
        let guard = if self.at_kw(Kw::If) {
            self.bump();
            let saved = self.in_guard;
            self.in_guard = true;
            let g = self.cond();
            self.in_guard = saved;
            Some(g?)
        } else {
            None
        };
        if !self.at(&Tok::FatArrow) {
            let sp = self.span();
            let colon = matches!(self.peek(), Tok::Colon);
            let d = self.error(sp, "expected `=>` after the pattern");
            if colon {
                d.help = Some("arms use `=>`: `pattern => value`".into());
            }
            return Err(());
        }
        self.bump();
        let body = if self.at(&Tok::LBrace) {
            self.block()?
        } else {
            let s = self.stmt()?;
            let sp = s.span;
            Block { stmts: vec![s], span: sp }
        };
        Ok(MatchArm { patterns, guard, body, span: start.to(self.prev_span()) })
    }

    fn pattern(&mut self) -> PResult<Pattern> {
        let sp = self.span();
        match self.peek().clone() {
            Tok::Ident(n) if n == "_" => {
                self.bump();
                Ok(Pattern::Wildcard(sp))
            }
            Tok::Ident(n) if n == "case" && !matches!(self.peek_at(1), Tok::FatArrow | Tok::Pipe) => {
                self.error6(sp, "`case` does not exist", "inside `match`, each arm is `pattern => …`");
                Err(())
            }
            Tok::Ident(n) => {
                self.bump();
                if self.at(&Tok::Dot) {
                    self.error(sp, "in `match` arms, enum cases are written without the type name").help =
                        Some("write only the case: `prod =>` instead of `Env.prod =>`".into());
                    return Err(());
                }
                if self.at(&Tok::LBrace) {
                    return self.struct_pattern(Ident { name: n, span: sp });
                }
                Ok(Pattern::Case(Ident { name: n, span: sp }))
            }
            Tok::Kw(Kw::True) | Tok::Kw(Kw::False) => {
                let b = self.at_kw(Kw::True);
                self.bump();
                Ok(Pattern::Lit(Expr { kind: ExprKind::Bool(b), span: sp }))
            }
            Tok::Str(s) => {
                self.bump();
                match s.plain() {
                    Some(t) => Ok(Pattern::Lit(Expr { kind: if s.raw { ExprKind::RawStr(t) } else { ExprKind::Str(vec![StrPart::Lit(t)]) }, span: sp })),
                    None => {
                        self.error(sp, "patterns are fixed values, without `{ }`");
                        Err(())
                    }
                }
            }
            Tok::Dur(d) => {
                self.bump();
                Ok(Pattern::Lit(Expr { kind: ExprKind::Duration(d), span: sp }))
            }
            Tok::Int(_) | Tok::Minus => {
                let from = self.pat_int()?;
                let inclusive = match self.peek() {
                    Tok::DotDot => false,
                    Tok::DotDotEq => true,
                    _ => return Ok(Pattern::Lit(Expr { kind: ExprKind::Int(from), span: sp.to(self.prev_span()) })),
                };
                self.bump();
                let to = self.pat_int()?;
                Ok(Pattern::Range { from, to, inclusive, span: sp.to(self.prev_span()) })
            }
            Tok::Kw(Kw::Else) => {
                self.error(sp, "the \"everything else\" pattern is `_`").help = Some("write `_ => …`".into());
                Err(())
            }
            _ => {
                self.unexpected("a pattern (literal, range `0..10`, enum case or `_`)")?;
                Err(())
            }
        }
    }

    /// `Name { field: pattern, … }` (one line or several; commas between fields).
    fn struct_pattern(&mut self, name: Ident) -> PResult<Pattern> {
        self.bump();
        let mut fields = Vec::new();
        self.skip_nl();
        while !matches!(self.peek(), Tok::RBrace | Tok::Eof) {
            let fsp = self.span();
            let Tok::Ident(f) = self.peek().clone() else {
                self.unexpected("a field name")?;
                return Err(());
            };
            self.bump();
            if !self.eat(&Tok::Colon) {
                self.error(self.span(), "expected `:` after the field").help = Some(format!("write `{}: pattern`, like `{}: 80` or `{}: x`", f, f, f));
                return Err(());
            }
            let p = self.pattern()?;
            fields.push((Ident { name: f, span: fsp }, p));
            self.skip_nl();
            if !self.eat(&Tok::Comma) {
                break;
            }
            self.skip_nl();
        }
        self.skip_nl();
        let end = self.expect(Tok::RBrace, "`}` at the end of the pattern")?;
        Ok(Pattern::Struct { span: name.span.to(end), name, fields })
    }

    fn pat_int(&mut self) -> PResult<i64> {
        let neg = self.eat(&Tok::Minus);
        match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                Ok(if neg { n.wrapping_neg() } else { n })
            }
            _ => {
                self.unexpected("an integer")?;
                Err(())
            }
        }
    }

    fn parallel(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let mut max = None;
        if self.at(&Tok::LParen) {
            let (args, _) = self.call_args()?;
            for a in args {
                match &a.name {
                    Some(n) if n.name == "max" && max.is_none() => max = Some(Box::new(a.value)),
                    Some(n) => {
                        self.diags.push(Diag::error("E012", n.span, format!("`parallel` has no parameter `{}`", n.name)).with_help("the form is `parallel(max: 4)`"));
                    }
                    None => {
                        self.diags.push(Diag::error("E012", a.value.span, "`parallel` only accepts `max:`").with_help("the form is `parallel(max: 4) for x in xs { … }`"));
                    }
                }
            }
        }
        let body = if self.at_kw(Kw::For) {
            self.bump();
            let var = self.ident("a loop variable name")?;
            if self.at(&Tok::Comma) {
                let sp = self.span();
                self.diags.push(Diag::error("E090", sp, "`parallel for` with an index is not supported yet").with_help("use `parallel for x in list`"));
                return Err(());
            }
            self.expect(Tok::Kw(Kw::In), "`in`")?;
            let iter = self.cond_expr()?;
            let body = self.block()?;
            ParBody::For { var, iter: Box::new(iter), body }
        } else if self.at(&Tok::LBrace) {
            ParBody::Block(self.block()?)
        } else {
            self.unexpected("`for` or `{` after `parallel`")?;
            return Err(());
        };
        Ok(Expr { kind: ExprKind::Parallel { max, body }, span: start.to(self.prev_span()) })
    }

    // ---------------------------------------------------------------------------------------------
    // Strings and commands
    // ---------------------------------------------------------------------------------------------

    fn snippet(&mut self, code: &str, offset: usize) -> Expr {
        if self.depth > MAX_DEPTH {
            self.diags.push(Diag::error("E005", Span::new(offset, offset + code.len()), "nesting too deep"));
            return Expr { kind: ExprKind::Error, span: Span::new(offset, offset + code.len()) };
        }
        let (e, mut d) = parse_expr_at(self.src, code, offset, self.depth + 1);
        self.diags.append(&mut d);
        e
    }

    fn str_expr(&mut self, s: &StrTok, sp: Span) -> Expr {
        if s.raw {
            return Expr { kind: ExprKind::RawStr(s.plain().unwrap_or_default()), span: sp };
        }
        let mut parts = Vec::new();
        for p in &s.parts {
            match p {
                LStrPart::Lit(t) => parts.push(StrPart::Lit(t.clone())),
                LStrPart::Code { src, offset } => {
                    let e = self.snippet(src, *offset);
                    parts.push(StrPart::Expr(e));
                }
            }
        }
        Expr { kind: ExprKind::Str(parts), span: sp }
    }

    fn word(&mut self, w: &LWord) -> CmdWord {
        let mut parts = Vec::new();
        for p in &w.parts {
            match p {
                WPart::Lit(t) => parts.push(WordPart::Lit(t.clone())),
                WPart::Code { src, offset, quoted } => {
                    let e = self.snippet(src, *offset);
                    parts.push(WordPart::Expr { expr: e, quoted: *quoted });
                }
            }
        }
        CmdWord { parts, span: w.span }
    }

    fn cmd_ast(&mut self, c: &CmdTok, sp: Span) -> CmdAst {
        let stages = c.stages.iter().map(|st| st.iter().map(|w| self.word(w)).collect()).collect();
        let redirects = c
            .redirects
            .iter()
            .map(|r| CmdRedir { kind: r.kind, target: r.target.as_ref().map(|w| self.word(w)), span: r.span })
            .collect();
        CmdAst { stages, redirects, stdin: None, text: c.text.clone(), span: sp }
    }
}

/// `$ cmd` in a condition becomes an exit-code test.
fn to_cmd_test(e: Expr) -> Expr {
    match e.kind {
        ExprKind::Command(c) if c.stdin.is_none() => Expr { kind: ExprKind::CmdTest(c), span: e.span },
        ExprKind::Unary(UnOp::Not, inner) => {
            let inner = to_cmd_test(*inner);
            Expr { kind: ExprKind::Unary(UnOp::Not, Box::new(inner)), span: e.span }
        }
        _ => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> File {
        let (f, d) = parse(src);
        assert!(d.is_empty(), "diags: {:#?}", d);
        f
    }
    fn codes(src: &str) -> Vec<&'static str> {
        parse(src).1.iter().map(|d| d.code).collect()
    }

    #[test]
    fn examples_parse() {
        for f in ["hello", "backup", "deploy", "installer", "release", "release_test"] {
            let path = format!("{}/examples/{}.nalu", env!("CARGO_MANIFEST_DIR"), f);
            let src = std::fs::read_to_string(&path).unwrap();
            let (_, d) = parse(&src);
            assert!(d.is_empty(), "{}: {:#?}", f, d);
        }
    }

    #[test]
    fn constructs() {
        let f = ok("let x = 1 + 2 * 3\nvar y: []str = []\ny += [\"a\"]\nfor i, v in xs { print(i) }\n");
        assert_eq!(f.items.len(), 4);
        let f = ok("let m = match x {\n  0..12 => \"a\"\n  12 | 13 => \"b\"\n  _ => fail \"c\"\n}\n");
        assert_eq!(f.items.len(), 1);
        ok("let s = Servico { ..base, porta: 1 }\nlet v = if a { 1 } else if b { 2 } else { 3 }\n");
        ok("let xs = ys.filter(y => y > 1).map((a) => { return a })\n");
        ok("try {\n $ git add x\n} catch fail \"x: {err}\"\n");
        ok("retry(3, every: 1s) {\n fail \"x\"\n} catch ignore\n");
        ok("let t = parallel(max: 4) for p in ps {\n f(p) catch 0\n}\n");
        ok("token | $ docker login --password-stdin\n");
        ok("if $ git diff --quiet {\n print(\"ok\")\n}\n");
        ok("native {\n bash: 'ulimit -n 1'\n powershell: 'x'\n}\n");
        ok("fail \"x\", code: 2\nexit(1)\nexit()\n");
        assert!(codes("fail(\"x\", code: 2)\n").contains(&"E006"));
        assert!(codes("print(x catch fail \"a\", 2)\n").is_empty(), "the comma without `code:` belongs to the argument");
        ok("let x = a\n    .trim()\n    .upper()\n");
        ok("let m = { \"a\": 1, \"b\": 2 }\nlet e = {}\n");
    }

    #[test]
    fn recovery_from_several_errors() {
        let src = "let = 1\nlet x = )\nfoo(]\nlet y = 2 2\nvar\nprint(1 2)\nlet z = 3\n";
        let (f, d) = parse(src);
        assert_eq!(d.len(), 6, "{:#?}", d);
        // the last statement was parsed
        assert!(f.items.iter().any(|i| matches!(i, Item::Stmt(Stmt { kind: StmtKind::Let { name, .. }, .. }) if name.name == "z")));
    }

    #[test]
    fn e006() {
        assert_eq!(codes("const x = 1"), vec!["E006"]);
        assert_eq!(codes("func f() {}"), vec!["E006"]);
        assert_eq!(codes("switch x {\n}"), vec!["E006"]);
        assert_eq!(codes("throw \"x\""), vec!["E006"]);
        assert_eq!(codes("x++"), vec!["E006"]);
        assert_eq!(codes("let x = a ?? b"), vec!["E006"]);
        assert_eq!(codes("for x of xs {\n}"), vec!["E006"]);
        assert_eq!(codes("import fs"), vec!["E006"]);
        assert_eq!(codes("let x = null"), vec!["E006"]);
        assert_eq!(codes("let x = a ? 1 : 2"), vec!["E006"]);
        assert_eq!(codes("print \"x\""), vec!["E006"]);
        assert_eq!(codes("fn f() throws {\n}"), vec!["E006"]);
        assert_eq!(codes("match x {\n case 1 => 2\n}"), vec!["E006"]);
        assert_eq!(codes("if x => 2 {\n}"), vec!["E006"]);
        assert_eq!(codes("let x = try f()"), vec!["E006"]);
    }

    #[test]
    fn no_panic_on_garbage() {
        let lixo = [
            "{", "}", "(", "fn", "fn f(", "struct S {", "match x {", "if", "let x = \"abc", "$(", "${", "\"{\"", "\"{{{\"",
            "let x = [1, 2", "enum E {", "x.", "x[", "parallel", "retry", "native {", "test", "a or", "a |", "for", "fail(",
            "let x = {a: }", "{{{{{{{{{{{{{{{{{{{{", "\u{0}", "let é = 1",
        ];
        for s in lixo {
            let _ = parse(s);
        }
        // as in `lower::compile`, which runs the frontend on a thread with a large stack
        std::thread::Builder::new().stack_size(64 << 20).spawn(deeply_nested).unwrap().join().unwrap();
    }

    fn deeply_nested() {
        let deep = "(".repeat(5000);
        let _ = parse(&deep);
        let deep = format!("let x = {}1{}", "[".repeat(3000), "]".repeat(3000));
        let _ = parse(&deep);
        let deep = format!("let x = \"{}\"", "{\"".repeat(200));
        let _ = parse(&deep);
    }
}
