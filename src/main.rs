//! Nalu CLI (SPEC-004): `run`, `build`, `check`, `emit`, `test`, `fmt`, `doc`, `ide`, `--version`, `--help`.
//!
//! Exit codes: 0 ok; 1 compile errors, failed tests or an unreadable file;
//! 2 wrong CLI usage. The exit code of the script run by `run`/`test` is passed through.
//!
//! `nalu test` runs the test **packages** of the folder: `x_test.nalu` together with `x.nalu` (the texts are
//! concatenated, the one that declares `param` first, because `param` can only appear at the start, with a
//! `// --- file` comment between them; an offset map returns each diagnostic to the right file),
//! or a file with its own `test` blocks. Joining the whole folder does not work: `examples/` has
//! four independent scripts, each with its own `param`s.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nalu::diag::{Diag, Severity, Span};
use nalu::ir;
use nalu::{Mode, Target};

const HELP: &str = "nalu: one wave, every shell (a compiler from one script to Bash and PowerShell)

usage:
  nalu run <file.nalu> [-- args…]                 compile for this system and run
  nalu build <file.nalu> [--target a,b] [-o dir]  write x.linux.sh, x.macos.sh, x.windows.ps1 and x.windows.cmd
  nalu check <file.nalu> [--json] [--explain]     check only: errors and warnings; --explain lists every line
                                                  that can fail without `catch` and what happens if it does
  nalu check --stdin <name.nalu> [--json] […]     the same, reading the source from standard input (editors)
  nalu emit <file.nalu> [--target T] [--ir]       print the generated script (or the IR)
  nalu test [folder|file] [--target T]            run the `test` blocks of the .nalu files in the folder
  nalu fmt <file|folder>… [--check]               format .nalu files in place (folders: every .nalu inside);
                                                  --check changes nothing, lists the files that would change
  nalu fmt --stdin <name.nalu>                    format standard input to standard output (editors)
  nalu doc [name…] [--all]                        documentation for a module, function, type or keyword
                                                  (`nalu doc fs`, `nalu doc str.split`); no name: the overview;
                                                  --all: the whole standard library reference, as markdown
  nalu ide <query> <file.nalu> --at LINE:COLUMN [--stdin]
                                                  editor queries, as JSON: hover, complete, signature,
                                                  definition, context (used by the VS Code extension)
  nalu --version | -V
  nalu --help | -h

targets: linux, macos (darwin, mac), windows (win)
exit codes: 0 ok; 1 compile errors or failed tests; 2 wrong usage";

// ---------------------------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Cmd {
    Help,
    Version,
    Run { file: PathBuf, args: Vec<String> },
    Build { file: PathBuf, targets: Vec<Target>, out: PathBuf },
    Check { file: PathBuf, json: bool, explain: bool, stdin: bool },
    Emit { file: PathBuf, target: Target, ir: bool },
    Test { path: PathBuf, target: Target },
    Ide { query: IdeQuery, file: PathBuf, stdin: bool, at: (usize, usize) },
    Fmt { paths: Vec<PathBuf>, check: bool, stdin: bool },
    Doc { names: Vec<String>, all: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum IdeQuery {
    Hover,
    Complete,
    Signature,
    Definition,
    Context,
}

/// A parsed flag: name and value (when it came as `--x=v`).
fn split_flag(a: &str) -> (&str, Option<&str>) {
    match a.find('=') {
        Some(i) if a.starts_with('-') => (&a[..i], Some(&a[i + 1..])),
        _ => (a, None),
    }
}

fn parse_targets(v: &str) -> Result<Vec<Target>, String> {
    let mut out = Vec::new();
    for part in v.split(',') {
        let part = part.trim();
        match Target::parse(part) {
            Some(t) => {
                if !out.contains(&t) {
                    out.push(t)
                }
            }
            None => return Err(format!("unknown target: `{}` (use linux, macos or windows)", part)),
        }
    }
    if out.is_empty() {
        return Err("--target needs at least one target".into());
    }
    Ok(out)
}

fn parse_args(argv: &[String]) -> Result<Cmd, String> {
    let Some(sub) = argv.first() else {
        return Err("a subcommand is missing".into());
    };
    let sub = sub.as_str();
    match sub {
        "-h" | "--help" | "help" => return Ok(Cmd::Help),
        "-V" | "--version" | "version" => return Ok(Cmd::Version),
        "run" | "build" | "check" | "emit" | "test" | "ide" | "fmt" | "doc" => {}
        _ => return Err(format!("unknown subcommand: `{}`", sub)),
    }

    let mut positional: Vec<String> = Vec::new();
    let mut script_args: Vec<String> = Vec::new();
    let mut targets: Option<Vec<Target>> = None;
    let mut out: Option<PathBuf> = None;
    let (mut json, mut explain, mut ir, mut stdin, mut fmt_check, mut doc_all) = (false, false, false, false, false, false);
    let mut at: Option<(usize, usize)> = None;

    let mut i = 1;
    while i < argv.len() {
        let a = argv[i].as_str();
        i += 1;
        if a == "--" {
            if sub != "run" {
                return Err(format!("`--` only works with `nalu run`"));
            }
            script_args.extend(argv[i..].iter().cloned());
            break;
        }
        if !a.starts_with('-') || a == "-" {
            positional.push(a.to_string());
            continue;
        }
        let (name, inline) = split_flag(a);
        // Reads the value of a flag that needs one: `--x v` or `--x=v`.
        let mut value = |what: &str| -> Result<String, String> {
            if let Some(v) = inline {
                return Ok(v.to_string());
            }
            match argv.get(i) {
                Some(v) if !v.starts_with('-') || v == "-" => {
                    i += 1;
                    Ok(v.clone())
                }
                _ => Err(format!("{} needs a value", what)),
            }
        };
        let no_value = |inline: Option<&str>| -> Result<(), String> {
            match inline {
                Some(_) => Err(format!("{} does not take a value", name)),
                None => Ok(()),
            }
        };
        match (sub, name) {
            (_, "-h") | (_, "--help") => return Ok(Cmd::Help),
            ("build", "--target") | ("emit", "--target") | ("test", "--target") => {
                if targets.is_some() {
                    return Err("--target given twice".into());
                }
                targets = Some(parse_targets(&value("--target")?)?);
            }
            ("build", "-o") | ("build", "--out") => {
                if out.is_some() {
                    return Err("-o given twice".into());
                }
                out = Some(PathBuf::from(value("-o")?));
            }
            ("check", "--json") => {
                no_value(inline)?;
                json = true
            }
            ("check", "--explain") => {
                no_value(inline)?;
                explain = true
            }
            ("check", "--stdin") | ("ide", "--stdin") | ("fmt", "--stdin") => {
                no_value(inline)?;
                stdin = true
            }
            ("doc", "--all") => {
                no_value(inline)?;
                doc_all = true
            }
            ("fmt", "--check") => {
                no_value(inline)?;
                fmt_check = true
            }
            ("ide", "--at") => {
                let v = value("--at")?;
                let parsed = v.split_once(':').and_then(|(l, c)| Some((l.parse::<usize>().ok()?, c.parse::<usize>().ok()?)));
                match parsed {
                    Some((l, c)) if l >= 1 && c >= 1 => at = Some((l, c)),
                    _ => return Err(format!("--at takes LINE:COLUMN, both from 1 (got '{}')", v)),
                }
            }
            ("emit", "--ir") => {
                no_value(inline)?;
                ir = true
            }
            _ => return Err(format!("unknown option for `nalu {}`: `{}`", sub, name)),
        }
    }

    let one_file = |positional: &[String]| -> Result<PathBuf, String> {
        match positional {
            [f] => Ok(PathBuf::from(f)),
            [] => Err(format!("`nalu {}` needs a .nalu file", sub)),
            _ => Err(format!("`nalu {}` takes a single file (pass script arguments after `--`)", sub)),
        }
    };

    let single_target = |targets: Option<Vec<Target>>| -> Result<Target, String> {
        match targets {
            None => Ok(Target::current()),
            Some(v) if v.len() == 1 => Ok(v[0]),
            Some(_) => Err(format!("`nalu {}` takes a single target", sub)),
        }
    };

    match sub {
        "run" => Ok(Cmd::Run { file: one_file(&positional)?, args: script_args }),
        "build" => Ok(Cmd::Build {
            file: one_file(&positional)?,
            targets: targets.unwrap_or_else(|| Target::ALL.to_vec()),
            out: out.unwrap_or_else(|| PathBuf::from("dist")),
        }),
        "check" => Ok(Cmd::Check { file: one_file(&positional)?, json, explain, stdin }),
        "emit" => Ok(Cmd::Emit { file: one_file(&positional)?, target: single_target(targets)?, ir }),
        "test" => {
            let path = match positional.as_slice() {
                [] => PathBuf::from("."),
                [p] => PathBuf::from(p),
                _ => return Err("`nalu test` takes a folder or a file".into()),
            };
            Ok(Cmd::Test { path, target: single_target(targets)? })
        }
        "ide" => {
            let (q, file) = match positional.as_slice() {
                [q, f] => (q.as_str(), PathBuf::from(f)),
                _ => return Err("usage: nalu ide <hover|complete|signature|definition|context> <file.nalu> --at LINE:COLUMN [--stdin]".into()),
            };
            let query = match q {
                "hover" => IdeQuery::Hover,
                "complete" => IdeQuery::Complete,
                "signature" => IdeQuery::Signature,
                "definition" => IdeQuery::Definition,
                "context" => IdeQuery::Context,
                _ => return Err(format!("unknown query `{}`; use hover, complete, signature, definition or context", q)),
            };
            let Some(at) = at else { return Err("`nalu ide` needs --at LINE:COLUMN".into()) };
            Ok(Cmd::Ide { query, file, stdin, at })
        }
        "doc" => Ok(Cmd::Doc { names: positional, all: doc_all }),
        "fmt" => {
            if stdin && fmt_check {
                return Err("`nalu fmt --stdin` does not take --check".into());
            }
            match (stdin, positional.as_slice()) {
                (true, []) => Err("`nalu fmt --stdin` needs the file name (used in messages)".into()),
                (false, []) => Err("`nalu fmt` needs a .nalu file or folder".into()),
                (true, [f]) => Ok(Cmd::Fmt { paths: vec![PathBuf::from(f)], check: false, stdin: true }),
                (true, _) => Err("`nalu fmt --stdin` takes a single file name".into()),
                (false, ps) => Ok(Cmd::Fmt { paths: ps.iter().map(PathBuf::from).collect(), check: fmt_check, stdin: false }),
            }
        }
        _ => unreachable!(),
    }
}

// ---------------------------------------------------------------------------------------------
// Source and diagnostics
// ---------------------------------------------------------------------------------------------

/// A source text with the offset map of each file that makes it up (just one, outside `test`).
struct Source {
    text: String,
    /// Name shown in diagnostics and passed to the frontend.
    file: String,
    embed_root: PathBuf,
    /// `(start in the text, file name, file text)`, in order.
    parts: Vec<(usize, String, String)>,
    /// Problems found while loading the files of `use` (E055), in the coordinates of `text`.
    extra: Vec<Diag>,
}

impl Source {
    fn single(path: &Path) -> Result<Source, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("could not read {}: {}", path.display(), io_msg(&e)))?;
        let text = text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text);
        let file = path.display().to_string();
        Ok(with_uses(Source {
            parts: vec![(0, file.clone(), text.clone())],
            text,
            file,
            embed_root: parent_dir(path),
            extra: Vec::new(),
        }))
    }

    /// The text comes from standard input; `path` only gives the name and the `embed` folder (editors, unsaved buffer).
    fn stdin(path: &Path) -> Result<Source, String> {
        use std::io::Read as _;
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map_err(|e| format!("could not read standard input: {}", io_msg(&e)))?;
        let text = text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text);
        let file = path.display().to_string();
        Ok(with_uses(Source { parts: vec![(0, file.clone(), text.clone())], text, file, embed_root: parent_dir(path), extra: Vec::new() }))
    }

    /// Renders diagnostics, returning each one to its source file.
    fn render(&self, diags: &[Diag]) -> String {
        let mut sorted: Vec<&Diag> = diags.iter().collect();
        sorted.sort_by_key(|d| (d.span.start, d.severity != Severity::Error));
        sorted.iter().map(|d| {
            let (src, file, local) = self.locate(d);
            local.render(src, file)
        }).collect::<Vec<_>>().join("\n")
    }

    fn render_json(&self, diags: &[Diag]) -> String {
        let items: Vec<String> = diags.iter().map(|d| {
            let (src, file, local) = self.locate(d);
            "  ".to_string() + &local.to_json(src, file)
        }).collect();
        if items.is_empty() {
            return "[]".to_string();
        }
        format!("[\n{}\n]", items.join(",\n"))
    }

    /// The source file of a span of the combined text: (file text, name, local span).
    fn locate_span<'a>(&'a self, sp: Span) -> (&'a str, &'a str, Span) {
        let idx = self.parts.iter().rposition(|(start, _, _)| *start <= sp.start).unwrap_or(0);
        let (start, file, text) = &self.parts[idx];
        (text.as_str(), file.as_str(), Span::new(sp.start.saturating_sub(*start), sp.end.saturating_sub(*start)))
    }

    fn locate<'a>(&'a self, d: &Diag) -> (&'a str, &'a str, Diag) {
        let idx = self.parts.iter().rposition(|(start, _, _)| *start <= d.span.start).unwrap_or(0);
        let (start, file, text) = &self.parts[idx];
        let mut local = d.clone();
        local.span = Span::new(d.span.start.saturating_sub(*start), d.span.end.saturating_sub(*start));
        (text.as_str(), file.as_str(), local)
    }
}

fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn io_msg(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "file not found".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        _ => e.to_string(),
    }
}

/// The frontend for one target, plus the problems of `use` (which make the program unusable).
fn compile_src(src: &Source, target: Target) -> nalu::Compiled {
    let mut c = nalu::compile(&src.text, &src.file, &src.embed_root, target);
    if !src.extra.is_empty() {
        c.diags.extend(src.extra.iter().cloned());
        if src.extra.iter().any(Diag::is_error) {
            c.program = None;
        }
    }
    c
}

/// `use "lib.nalu"` (D37): the files used, found recursively relative to the file that uses them and loaded
/// once, are appended to the source, so the script still compiles to one file per target. A used file can
/// only declare `fn`, `struct`, `enum` and `test`. In every file but the first, the `use` lines are blanked
/// out (same length, so positions don't move): they already did their job, and `use` belongs at the start.
fn with_uses(src: Source) -> Source {
    use nalu::ast::Item;
    // (name, text, folder, used by another file)
    let mut files: Vec<(String, String, PathBuf, bool)> = src.parts.iter().map(|(_, n, t)| (n.clone(), t.clone(), parent_dir(Path::new(n)), false)).collect();
    let mut seen: Vec<PathBuf> = files.iter().map(|(n, _, _, _)| Path::new(n).canonicalize().unwrap_or_else(|_| PathBuf::from(n))).collect();
    // (file index, local span, diagnostic)
    let mut problems: Vec<(usize, Diag)> = Vec::new();
    let mut uses_anywhere = false;
    let mut i = 0;
    while i < files.len() {
        let (ast, _) = nalu::parser::parse(&files[i].1);
        for item in &ast.items {
            match item {
                Item::Use(path, span) => {
                    uses_anywhere = true;
                    let full = files[i].2.join(path);
                    match full.canonicalize() {
                        Ok(c) if seen.contains(&c) => {}
                        Ok(c) => match read_source(&c) {
                            Ok(text) => {
                                seen.push(c);
                                files.push((full.display().to_string(), text, parent_dir(&full), true));
                            }
                            Err(e) => problems.push((i, Diag::error("E055", *span, e))),
                        },
                        Err(_) => problems.push((
                            i,
                            Diag::error("E055", *span, format!("`{}` does not exist (looked for at `{}`)", path, full.display()))
                                .with_help("the path is relative to the folder of the file with the `use`"),
                        )),
                    }
                }
                Item::Fn(_) | Item::Struct(_) | Item::Enum(_) | Item::Test(_) => {}
                other if files[i].3 => {
                    let span = match other {
                        Item::Param(p) => p.span,
                        Item::Stmt(st) => st.span,
                        _ => Span::new(0, 0),
                    };
                    problems.push((
                        i,
                        Diag::error("E055", span, "a file used with `use` can only declare `fn`, `struct`, `enum` and `test`")
                            .with_help("move the `param` lines and the code to the script that uses this file"),
                    ));
                }
                _ => {}
            }
        }
        i += 1;
    }
    if !uses_anywhere {
        return src;
    }
    // the combined text again, with the used files appended
    let mut text = String::new();
    let mut parts = Vec::new();
    for (k, (name, t, _, _)) in files.iter().enumerate() {
        if !text.is_empty() {
            text.push_str(&format!("\n// --- {}\n", name));
        }
        let mut body = t.clone();
        if k > 0 {
            let (ast, _) = nalu::parser::parse(t);
            for item in &ast.items {
                if let Item::Use(_, sp) = item {
                    let blank: String = t[sp.start..sp.end].chars().map(|c| if c == '\n' { '\n' } else { ' ' }).collect();
                    body.replace_range(sp.start..sp.end, &blank);
                }
            }
        }
        parts.push((text.len(), name.clone(), t.clone()));
        text.push_str(&body);
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }
    let extra = problems
        .into_iter()
        .map(|(k, mut d)| {
            let start = parts[k].0;
            d.span = Span::new(d.span.start + start, d.span.end + start);
            d
        })
        .collect();
    Source { text, file: src.file, embed_root: src.embed_root, parts, extra }
}

/// Compiles for one target. Warnings go to stderr (once per call); errors abort with `Err(1)`.
fn compile_for(src: &Source, target: Target, quiet_warnings: bool) -> Result<ir::Program, u8> {
    let c = compile_src(src, target);
    let has_error = c.diags.iter().any(Diag::is_error);
    if !c.diags.is_empty() && (has_error || !quiet_warnings) {
        eprintln!("{}", src.render(&c.diags));
    }
    match c.program {
        Some(p) if !has_error => Ok(p),
        _ => {
            if !has_error {
                eprintln!("nalu: the frontend produced no program for {} (and no diagnostic)", target.os_name());
            }
            Err(1)
        }
    }
}

/// Merges the diagnostics of the three targets, without repeats.
fn diags_all_targets(src: &Source) -> (Vec<Diag>, Vec<(Target, ir::Program)>) {
    let mut all: Vec<Diag> = Vec::new();
    let mut programs = Vec::new();
    for t in Target::ALL {
        let c = compile_src(src, t);
        for d in c.diags {
            if !all.iter().any(|x| x == &d) {
                all.push(d);
            }
        }
        if let Some(p) = c.program {
            programs.push((t, p));
        }
    }
    all.sort_by_key(|d| (d.span.start, d.severity != Severity::Error));
    (all, programs)
}

// ---------------------------------------------------------------------------------------------
// Generation and writing
// ---------------------------------------------------------------------------------------------

fn generate(program: &ir::Program, mode: Mode) -> String {
    if program.target.is_bash() {
        nalu::bash::generate(program, mode)
    } else {
        nalu::ps::generate(program, mode)
    }
}

/// The final `.ps1`: CRLF (without doubling a `\r\n` that is already there). The generator writes only ASCII, so there is no BOM
/// (`irm … | iex` would break with it); only a `native` block with accented characters still needs the UTF-8 BOM, so that
/// Windows PowerShell 5.1 does not read the file as ANSI.
fn ps1_bytes(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + text.len() / 20 + 3);
    if !text.is_ascii() {
        out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' && (i == 0 || bytes[i - 1] != b'\r') {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

fn write_script(path: &Path, target: Target, text: &str) -> Result<(), String> {
    let res = if target.is_bash() {
        std::fs::write(path, text.as_bytes()).and_then(|_| make_executable(path))
    } else {
        std::fs::write(path, ps1_bytes(text))
    };
    res.map_err(|e| format!("could not write {}: {}", path.display(), io_msg(&e)))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "script".into())
}

// ---------------------------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------------------------

fn find_in_path(prog: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) { vec![".exe".into(), String::new()] } else { vec![String::new()] };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let p = dir.join(format!("{}{}", prog, ext));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn find_bash() -> Option<PathBuf> {
    let fixed = Path::new("/bin/bash");
    if fixed.is_file() {
        return Some(fixed.to_path_buf());
    }
    find_in_path("bash")
}

fn find_powershell() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NALU_PWSH") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    find_in_path("pwsh").or_else(|| find_in_path("powershell"))
}

/// Generates into a temporary file, runs it passing stdin/stdout/stderr through, and returns the script's exit code.
fn execute(program: &ir::Program, mode: Mode, name: &str, args: &[String]) -> Result<u8, String> {
    let target = program.target;
    let text = generate(program, mode);
    let dir = std::env::temp_dir().join(format!("nalu-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {}", dir.display(), io_msg(&e)))?;
    let script = dir.join(format!("{}.{}", name, target.file_suffix()));
    write_script(&script, target, &text)?;

    let mut cmd = if target.is_bash() {
        let bash = find_bash().ok_or("bash not found (looked in /bin/bash and PATH)")?;
        let mut c = std::process::Command::new(bash);
        c.arg(&script);
        c
    } else {
        let ps = find_powershell().ok_or("found neither pwsh nor powershell; set NALU_PWSH")?;
        let mut c = std::process::Command::new(ps);
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&script);
        c
    };
    cmd.args(args);
    let status = cmd.status();
    let _ = std::fs::remove_dir_all(&dir);
    let status = status.map_err(|e| format!("could not run the script: {}", e))?;
    Ok(exit_code_of(&status))
}

#[cfg(unix)]
fn exit_code_of(status: &std::process::ExitStatus) -> u8 {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(c), _) => (c & 0xff) as u8,
        (None, Some(sig)) => (128 + sig).min(255) as u8,
        _ => 1,
    }
}

#[cfg(not(unix))]
fn exit_code_of(status: &std::process::ExitStatus) -> u8 {
    status.code().map(|c| (c & 0xff) as u8).unwrap_or(1)
}

// ---------------------------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------------------------

fn cmd_run(file: &Path, args: &[String]) -> Result<u8, String> {
    let src = Source::single(file)?;
    let program = match compile_for(&src, Target::current(), false) {
        Ok(p) => p,
        Err(c) => return Ok(c),
    };
    execute(&program, Mode::Script, &stem(file), args)
}

fn cmd_build(file: &Path, targets: &[Target], out: &Path) -> Result<u8, String> {
    let src = Source::single(file)?;
    let name = stem(file);
    // Compiles all targets and shows the diagnostics once, together and without repeats (a W010 may belong to
    // a single target, like `grep` on Windows).
    let mut programs = Vec::new();
    let mut diags: Vec<Diag> = Vec::new();
    for &t in targets {
        let c = compile_src(&src, t);
        for d in c.diags {
            if !diags.contains(&d) {
                diags.push(d);
            }
        }
        if let Some(p) = c.program {
            programs.push(p);
        }
    }
    diags.sort_by_key(|d| (d.span.start, d.severity != Severity::Error));
    if !diags.is_empty() {
        eprintln!("{}", src.render(&diags));
    }
    if diags.iter().any(Diag::is_error) || programs.len() != targets.len() {
        return Ok(1);
    }
    std::fs::create_dir_all(out).map_err(|e| format!("could not create {}: {}", out.display(), io_msg(&e)))?;
    for p in &programs {
        let path = out.join(format!("{}.{}", name, p.target.file_suffix()));
        write_script(&path, p.target, &generate(p, Mode::Script))?;
        if p.target == Target::Windows {
            let cmd_path = out.join(format!("{}.windows.cmd", name));
            std::fs::write(&cmd_path, nalu::ps::launcher(&name))
                .map_err(|e| format!("could not write {}: {}", cmd_path.display(), io_msg(&e)))?;
        }
    }
    Ok(0)
}

fn cmd_check(file: &Path, json: bool, explain: bool, stdin: bool) -> Result<u8, String> {
    let src = check_source(file, stdin)?;
    let (mut diags, _) = diags_all_targets(&src);
    // With --stdin (editors), only what belongs to the open file; the rest of the package has its own editor.
    let me = file.display().to_string();
    if stdin {
        diags.retain(|d| src.locate_span(d.span).1 == me);
    }
    let has_error = diags.iter().any(Diag::is_error);
    let mut explained = if explain && !has_error { nalu::lower::explain(&src.text, &src.embed_root).ok() } else { None };
    if let (true, Some(e)) = (stdin, explained.as_mut()) {
        e.points.retain(|p| src.locate_span(p.span).1 == me);
    }
    if json {
        match &explained {
            // with --explain, an object: the diagnostics and the points that can fail
            Some(e) => out(&format!("{}\n", explain_json(&src, &src.render_json(&diags), e))),
            None => out(&format!("{}\n", src.render_json(&diags))),
        }
    } else {
        if !diags.is_empty() {
            eprintln!("{}", src.render(&diags));
        }
        if let Some(e) = &explained {
            out(&explain_text(&src, e));
        }
    }
    if has_error {
        return Ok(1);
    }
    if !json && diags.is_empty() && !explain {
        eprintln!("ok: {}", src.file);
    }
    Ok(0)
}

/// `file:line:column: `x` can fail → effect`, one per point, and a summary at the end.
fn explain_text(src: &Source, e: &nalu::lower::Explained) -> String {
    let mut t = String::new();
    for p in &e.points {
        let (text, file, sp) = src.locate_span(p.span);
        let (line, col) = nalu::diag::line_col(text, sp.start);
        // a `fail` does not "can fail": it fails, on purpose
        let verb = if p.what == "`fail`" { "fails" } else { "can fail" };
        t.push_str(&format!("{}:{}:{}: {} {} → {}\n", file, line, col, p.what, verb, p.effect));
    }
    match e.points.len() {
        0 => t.push_str("nothing can fail without `catch`\n"),
        1 => t.push_str("1 place can fail without `catch`\n"),
        n => t.push_str(&format!("{} places can fail without `catch`\n", n)),
    }
    if !e.fallible_fns.is_empty() {
        t.push_str(&format!("functions that can fail: {}\n", e.fallible_fns.join(", ")));
    }
    t
}

fn explain_json(src: &Source, diags_json: &str, e: &nalu::lower::Explained) -> String {
    use nalu::diag::{json_str, line_col};
    let points: Vec<String> = e
        .points
        .iter()
        .map(|p| {
            let (text, file, sp) = src.locate_span(p.span);
            let (l, c) = line_col(text, sp.start);
            let (el, ec) = line_col(text, sp.end);
            format!(
                "    {{\"file\":{},\"line\":{},\"column\":{},\"end_line\":{},\"end_column\":{},\"what\":{},\"effect\":{}}}",
                json_str(file), l, c, el, ec, json_str(&p.what), json_str(&p.effect)
            )
        })
        .collect();
    let fns: Vec<String> = e.fallible_fns.iter().map(|f| json_str(f)).collect();
    let points = if points.is_empty() { "[]".to_string() } else { format!("[\n{}\n  ]", points.join(",\n")) };
    format!("{{\n  \"diagnostics\": {},\n  \"fallible\": {},\n  \"fallible_fns\": [{}]\n}}", diags_json.replace("\n", "\n  "), points, fns.join(", "))
}

fn cmd_emit(file: &Path, target: Target, ir: bool) -> Result<u8, String> {
    let src = Source::single(file)?;
    let program = match compile_for(&src, target, false) {
        Ok(p) => p,
        Err(c) => return Ok(c),
    };
    if ir {
        out(&format!("{:#?}\n", program));
    } else {
        out(&generate(&program, Mode::Script));
    }
    Ok(0)
}

/// The `.nalu` files of a folder, in alphabetical order.
fn package_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("could not read folder {}: {}", dir.display(), io_msg(&e)))?;
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().map(|e| e == "nalu").unwrap_or(false))
        .collect();
    files.sort();
    Ok(files)
}

fn read_source(f: &Path) -> Result<String, String> {
    let t = std::fs::read_to_string(f).map_err(|e| format!("could not read {}: {}", f.display(), io_msg(&e)))?;
    Ok(t.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(t))
}

fn declares_param(text: &str) -> bool {
    text.lines().any(|l| l.trim_start().starts_with("param "))
}

fn has_tests(text: &str) -> bool {
    text.lines().any(|l| l.trim_start().starts_with("test \""))
}

/// The test packages of a folder (or of the given file). A package is `x_test.nalu` together with `x.nalu`
/// (if it exists), or a file that has its own `test` blocks. The files of a package are
/// concatenated (the one that declares `param` first) with `// --- file` between them.
fn test_packages(path: &Path) -> Result<Vec<Vec<PathBuf>>, String> {
    let (dir, only): (PathBuf, Option<PathBuf>) = if path.is_dir() {
        (path.to_path_buf(), None)
    } else if path.is_file() {
        // the path as the folder listing writes it (`sizes.nalu` → `./sizes.nalu`), for the comparisons
        let dir = parent_dir(path);
        let only = path.file_name().map(|n| dir.join(n)).unwrap_or_else(|| path.to_path_buf());
        (dir, Some(only))
    } else {
        return Err(format!("{} not found", path.display()));
    };
    let files = package_files(&dir)?;
    let mut packages: Vec<Vec<PathBuf>> = Vec::new();
    for f in &files {
        let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let text = read_source(f)?;
        let pkg = if let Some(base) = stem.strip_suffix("_test") {
            let b = dir.join(format!("{}.nalu", base));
            if b.is_file() { vec![b, f.clone()] } else { vec![f.clone()] }
        } else if has_tests(&text) {
            vec![f.clone()]
        } else {
            continue;
        };
        if let Some(o) = &only {
            let partner = dir.join(format!("{}_test.nalu", stem));
            if !pkg.contains(o) && !(pkg.len() == 2 && pkg[0] == *o && partner == pkg[1]) {
                continue;
            }
        }
        packages.push(pkg);
    }
    Ok(packages)
}

fn package_source(files: &[PathBuf]) -> Result<Source, String> {
    let mut loaded = Vec::new();
    for f in files {
        loaded.push((f.display().to_string(), read_source(f)?));
    }
    Ok(package_from(loaded, &files[0]))
}

/// A package from the texts (name, text), with the file that declares `param` first.
fn package_from(mut loaded: Vec<(String, String)>, first: &Path) -> Source {
    // `param` can only come at the start of the program: the file that has `param` goes first.
    loaded.sort_by_key(|(_, t)| !declares_param(t));
    let mut text = String::new();
    let mut parts = Vec::new();
    for (name, t) in loaded {
        if !text.is_empty() {
            text.push_str(&format!("\n// --- {}\n", name));
        }
        parts.push((text.len(), name, t.clone()));
        text.push_str(&t);
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }
    let file = first.display().to_string();
    with_uses(Source { text, file, embed_root: parent_dir(first), parts, extra: Vec::new() })
}

/// What `check` analyzes: the file alone or, for `x_test.nalu` with an `x.nalu` next to it, the whole
/// package, like `nalu test` (without this, the names from `x.nalu` would give E001 in the test file).
/// With `--stdin`, the file's text comes from standard input and the rest of the package from disk.
fn check_source(file: &Path, stdin: bool) -> Result<Source, String> {
    let main = if stdin { Source::stdin(file)? } else { Source::single(file)? };
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let Some(stem) = name.strip_suffix("_test.nalu") else { return Ok(main) };
    let sibling = file.with_file_name(format!("{}.nalu", stem));
    if !sibling.is_file() {
        return Ok(main);
    }
    let loaded = vec![(sibling.display().to_string(), read_source(&sibling)?), (main.file.clone(), main.text.clone())];
    Ok(package_from(loaded, file))
}

fn cmd_test(path: &Path, target: Target) -> Result<u8, String> {
    let packages = test_packages(path)?;
    if packages.is_empty() {
        eprintln!("nalu: no `test` blocks in {}", path.display());
        return Ok(1);
    }
    let mut worst = 0u8;
    for pkg in packages {
        let src = package_source(&pkg)?;
        let code = match compile_for(&src, target, false) {
            Ok(program) => {
                let name = program.name.clone();
                execute(&program, Mode::Tests, &name, &[])?
            }
            Err(c) => c,
        };
        worst = worst.max(code);
    }
    Ok(worst)
}

// ---------------------------------------------------------------------------------------------
// fmt
// ---------------------------------------------------------------------------------------------

/// The `.nalu` files of a folder, recursively and in order, skipping `target`, `dist` and hidden folders (and without
/// following symbolic links to folders).
fn fmt_files(dir: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("could not read folder {}: {}", dir.display(), io_msg(&e)))?;
    let mut entries: Vec<std::fs::DirEntry> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if !(name.starts_with('.') || name == "target" || name == "dist") {
                fmt_files(&path, found)?;
            }
        } else if path.extension().map_or(false, |x| x == "nalu") && path.is_file() {
            found.push(path);
        }
    }
    Ok(())
}

/// `nalu fmt`: rewrites in place (only the files that change), `--check` only lists the ones that would change and
/// `--stdin` formats standard input to standard output. Broken syntax: diagnostics on stderr,
/// nothing written, code 1.
fn cmd_fmt(paths: &[PathBuf], check: bool, stdin: bool) -> Result<u8, String> {
    let bad_syntax = |diags: &[Diag], text: &str, file: &str| {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        eprintln!("{}", nalu::diag::render_all(diags, text, file));
    };
    if stdin {
        use std::io::Read as _;
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map_err(|e| format!("could not read standard input: {}", io_msg(&e)))?;
        return match nalu::fmt::format(&text) {
            Ok(formatted) => {
                out(&formatted);
                Ok(0)
            }
            Err(diags) => {
                bad_syntax(&diags, &text, &paths[0].display().to_string());
                Ok(1)
            }
        };
    }
    let mut files = Vec::new();
    for p in paths {
        if p.is_dir() {
            fmt_files(p, &mut files)?;
        } else if p.is_file() {
            files.push(p.clone());
        } else {
            return Err(format!("{} not found", p.display()));
        }
    }
    let mut failed = false;
    for f in &files {
        let text = match std::fs::read_to_string(f) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("nalu: could not read {}: {}", f.display(), io_msg(&e));
                failed = true;
                continue;
            }
        };
        match nalu::fmt::format(&text) {
            Ok(formatted) if formatted == text => {}
            Ok(_) if check => {
                out(&format!("{}\n", f.display()));
                failed = true;
            }
            Ok(formatted) => {
                if let Err(e) = std::fs::write(f, formatted.as_bytes()) {
                    eprintln!("nalu: could not write {}: {}", f.display(), io_msg(&e));
                    failed = true;
                }
            }
            Err(diags) => {
                bad_syntax(&diags, &text, &f.display().to_string());
                failed = true;
            }
        }
    }
    Ok(if failed { 1 } else { 0 })
}

// ---------------------------------------------------------------------------------------------
// Documentation (`nalu doc`)
// ---------------------------------------------------------------------------------------------

/// One page per name (markdown); with no name, the summary. Unknown name: a message with a suggestion, code 1.
fn cmd_doc(names: &[String], all: bool) -> Result<u8, String> {
    if all {
        out(&nalu::ide::stdlib_reference());
        return Ok(0);
    }
    if names.is_empty() {
        out(&format!("{}\n", nalu::ide::doc("").unwrap_or_default()));
        return Ok(0);
    }
    let mut pages = Vec::new();
    let mut code = 0;
    for n in names {
        match nalu::ide::doc(n) {
            Ok(p) => pages.push(p),
            Err(e) => {
                eprintln!("nalu: {}", e);
                code = 1;
            }
        }
    }
    if !pages.is_empty() {
        out(&format!("{}\n", pages.join("\n\n")));
    }
    Ok(code)
}

// ---------------------------------------------------------------------------------------------
// Editors (`nalu ide`)
// ---------------------------------------------------------------------------------------------

/// Byte offset of `line:column` (both starting at 1; the column counts characters, as in
/// `check --json`). Outside the text, the end of the line (or of the text).
fn offset_of(text: &str, line: usize, col: usize) -> usize {
    let mut start = 0;
    for _ in 1..line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len(),
        }
    }
    let end = text[start..].find('\n').map(|i| start + i).unwrap_or(text.len());
    text[start..end].char_indices().nth(col - 1).map(|(i, _)| start + i).unwrap_or(end)
}

/// `"file":…,"line":…,"column":…,"end_line":…,"end_column":…` of a span of the combined text.
fn range_json(src: &Source, sp: Span) -> String {
    use nalu::diag::{json_str, line_col};
    let (text, file, local) = src.locate_span(sp);
    let (l, c) = line_col(text, local.start);
    let (el, ec) = line_col(text, local.end);
    format!("\"file\":{},\"line\":{},\"column\":{},\"end_line\":{},\"end_column\":{}", json_str(file), l, c, el, ec)
}

fn cmd_ide(query: IdeQuery, file: &Path, stdin: bool, at: (usize, usize)) -> Result<u8, String> {
    use nalu::diag::json_str;
    use nalu::ide;
    let src = check_source(file, stdin)?;
    let me = file.display().to_string();
    let (start, _, text) = src.parts.iter().find(|(_, f, _)| *f == me).unwrap_or(&src.parts[0]);
    let offset = start + offset_of(text, at.0, at.1);
    let json = match query {
        IdeQuery::Hover => match ide::hover(&src.text, &src.embed_root, offset) {
            Some(h) => format!("{{{},\"markdown\":{}}}", range_json(&src, h.span), json_str(&h.markdown)),
            None => "null".into(),
        },
        IdeQuery::Definition => match ide::definition(&src.text, &src.embed_root, offset) {
            Some(sp) => format!("{{{}}}", range_json(&src, sp)),
            None => "null".into(),
        },
        IdeQuery::Signature => match ide::signature(&src.text, &src.embed_root, offset) {
            Some(h) => {
                let ps: Vec<String> = h.params.iter().map(|(a, b)| format!("[{},{}]", a, b)).collect();
                format!("{{\"label\":{},\"params\":[{}],\"active\":{},\"doc\":{}}}", json_str(&h.label), ps.join(","), h.active, json_str(&h.doc))
            }
            None => "null".into(),
        },
        IdeQuery::Context => {
            let c = ide::ai_context(&src.text, &src.embed_root, offset);
            let traits: Vec<String> = c.traits.iter().map(|(n, v)| format!("{{\"name\":{},\"value\":{}}}", json_str(n), json_str(v))).collect();
            let snippets: Vec<String> = c
                .snippets
                .iter()
                .map(|(n, code, imp)| format!("{{\"name\":{},\"code\":{},\"importance\":{}}}", json_str(n), json_str(code), imp))
                .collect();
            format!("{{\"traits\":[{}],\"snippets\":[{}]}}", traits.join(","), snippets.join(","))
        }
        IdeQuery::Complete => {
            let c = ide::complete(&src.text, &src.embed_root, offset);
            let items: Vec<String> = c
                .items
                .iter()
                .map(|i| {
                    format!(
                        "    {{\"label\":{},\"kind\":\"{}\",\"detail\":{},\"doc\":{},\"insert\":{}}}",
                        json_str(&i.label),
                        i.kind,
                        json_str(&i.detail),
                        json_str(&i.doc),
                        i.insert.as_deref().map(json_str).unwrap_or_else(|| "null".into())
                    )
                })
                .collect();
            let (text, _, local) = src.locate_span(Span::new(c.from, c.from));
            let (l, col) = nalu::diag::line_col(text, local.start);
            format!("{{\"line\":{},\"column\":{},\"items\":[\n{}\n]}}", l, col, items.join(",\n"))
        }
    };
    out(&format!("{}\n", json));
    Ok(0)
}

// ---------------------------------------------------------------------------------------------

/// Writes to stdout ignoring write errors (`nalu emit x | head` must not panic).
fn out(text: &str) {
    use std::io::Write;
    let mut so = std::io::stdout().lock();
    let _ = so.write_all(text.as_bytes());
    let _ = so.flush();
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("nalu: {}\nrun nalu --help to see the commands", msg);
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match parse_args(&argv) {
        Ok(c) => c,
        Err(e) => return usage_error(&e),
    };
    let res = match &cmd {
        Cmd::Help => {
            out(&format!("{}\n", HELP));
            Ok(0)
        }
        Cmd::Version => {
            out(&format!("nalu {}\n", env!("CARGO_PKG_VERSION")));
            Ok(0)
        }
        Cmd::Run { file, args } => cmd_run(file, args),
        Cmd::Build { file, targets, out } => cmd_build(file, targets, out),
        Cmd::Check { file, json, explain, stdin } => cmd_check(file, *json, *explain, *stdin),
        Cmd::Emit { file, target, ir } => cmd_emit(file, *target, *ir),
        Cmd::Test { path, target } => cmd_test(path, *target),
        Cmd::Ide { query, file, stdin, at } => cmd_ide(*query, file, *stdin, *at),
        Cmd::Fmt { paths, check, stdin } => cmd_fmt(paths, *check, *stdin),
        Cmd::Doc { names, all } => cmd_doc(names, *all),
    };
    match res {
        Ok(code) => ExitCode::from(code),
        Err(msg) => {
            eprintln!("nalu: {}", msg);
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn basic_subcommands() {
        assert_eq!(parse_args(&args("--version")), Ok(Cmd::Version));
        assert_eq!(parse_args(&args("-V")), Ok(Cmd::Version));
        assert_eq!(parse_args(&args("-h")), Ok(Cmd::Help));
        assert_eq!(parse_args(&args("check x.nalu --help")), Ok(Cmd::Help));
        assert!(parse_args(&args("foo")).is_err());
        assert!(parse_args(&[]).is_err());
    }

    #[test]
    fn run_with_arguments() {
        assert_eq!(
            parse_args(&args("run x.nalu -- --nome Ana --vezes 2")),
            Ok(Cmd::Run { file: "x.nalu".into(), args: args("--nome Ana --vezes 2") })
        );
        assert_eq!(parse_args(&args("run x.nalu")), Ok(Cmd::Run { file: "x.nalu".into(), args: vec![] }));
        // without `--`, a script flag is unknown to the CLI
        assert!(parse_args(&args("run x.nalu --nome Ana")).is_err());
        assert!(parse_args(&args("run")).is_err());
        assert!(parse_args(&args("run a.nalu b.nalu")).is_err());
    }

    #[test]
    fn build_targets_and_output() {
        assert_eq!(
            parse_args(&args("build x.nalu --target linux,win -o out")),
            Ok(Cmd::Build { file: "x.nalu".into(), targets: vec![Target::Linux, Target::Windows], out: "out".into() })
        );
        assert_eq!(
            parse_args(&args("build --target=macos x.nalu --out=o")),
            Ok(Cmd::Build { file: "x.nalu".into(), targets: vec![Target::Macos], out: "o".into() })
        );
        assert_eq!(
            parse_args(&args("build x.nalu")),
            Ok(Cmd::Build { file: "x.nalu".into(), targets: Target::ALL.to_vec(), out: "dist".into() })
        );
        assert!(parse_args(&args("build x.nalu --target bsd")).is_err());
        assert!(parse_args(&args("build x.nalu --target")).is_err());
        assert!(parse_args(&args("build x.nalu -o")).is_err());
        assert!(parse_args(&args("build x.nalu --json")).is_err());
    }

    #[test]
    fn fmt_arguments() {
        assert_eq!(parse_args(&args("fmt a.nalu examples")), Ok(Cmd::Fmt { paths: vec!["a.nalu".into(), "examples".into()], check: false, stdin: false }));
        assert_eq!(parse_args(&args("fmt --check .")), Ok(Cmd::Fmt { paths: vec![".".into()], check: true, stdin: false }));
        assert_eq!(parse_args(&args("fmt --stdin x.nalu")), Ok(Cmd::Fmt { paths: vec!["x.nalu".into()], check: false, stdin: true }));
        assert!(parse_args(&args("fmt")).is_err());
        assert!(parse_args(&args("fmt --check")).is_err());
        assert!(parse_args(&args("fmt --stdin")).is_err());
        assert!(parse_args(&args("fmt --stdin a.nalu b.nalu")).is_err());
        assert!(parse_args(&args("fmt --stdin --check a.nalu")).is_err());
        assert!(parse_args(&args("fmt --check=1 a.nalu")).is_err());
        assert!(parse_args(&args("fmt a.nalu --json")).is_err());
        assert!(parse_args(&args("check --check a.nalu")).is_err());
        assert_eq!(parse_args(&args("fmt a.nalu -h")), Ok(Cmd::Help));
    }

    #[test]
    fn ide() {
        assert_eq!(
            parse_args(&args("ide hover a/x.nalu --stdin --at 3:7")),
            Ok(Cmd::Ide { query: IdeQuery::Hover, file: "a/x.nalu".into(), stdin: true, at: (3, 7) })
        );
        assert!(parse_args(&args("ide hover a.nalu")).is_err());
        assert_eq!(parse_args(&args("ide context a.nalu --at 1:1")), Ok(Cmd::Ide { query: IdeQuery::Context, file: "a.nalu".into(), stdin: false, at: (1, 1) }));
        assert_eq!(parse_args(&args("doc fs str.split")), Ok(Cmd::Doc { names: vec!["fs".into(), "str.split".into()], all: false }));
        assert_eq!(parse_args(&args("doc")), Ok(Cmd::Doc { names: vec![], all: false }));
        assert_eq!(parse_args(&args("doc --all")), Ok(Cmd::Doc { names: vec![], all: true }));
        assert!(parse_args(&args("ide nada a.nalu --at 1:1")).is_err());
        assert!(parse_args(&args("ide hover a.nalu --at 0:1")).is_err());
        assert_eq!(offset_of("ab\ncdé f\n", 2, 4), 7);
        assert_eq!(offset_of("ab\ncdé f\n", 2, 99), 9);
        assert_eq!(offset_of("ab", 9, 1), 2);
    }

    #[test]
    fn check_emit_test() {
        assert_eq!(parse_args(&args("check --json x.nalu --explain")), Ok(Cmd::Check { file: "x.nalu".into(), json: true, explain: true, stdin: false }));
        assert_eq!(parse_args(&args("check --stdin a/x.nalu --json")), Ok(Cmd::Check { file: "a/x.nalu".into(), json: true, explain: false, stdin: true }));
        assert!(parse_args(&args("check x.nalu --json=1")).is_err());
        assert_eq!(parse_args(&args("emit x.nalu --target windows --ir")), Ok(Cmd::Emit { file: "x.nalu".into(), target: Target::Windows, ir: true }));
        assert!(parse_args(&args("emit x.nalu --target linux,macos")).is_err());
        assert_eq!(parse_args(&args("test")), Ok(Cmd::Test { path: ".".into(), target: Target::current() }));
        assert_eq!(parse_args(&args("test examples --target=win")), Ok(Cmd::Test { path: "examples".into(), target: Target::Windows }));
        assert!(parse_args(&args("test a b")).is_err());
        assert!(parse_args(&args("check x.nalu -- a")).is_err());
    }

    #[test]
    fn ps1_crlf_and_bom_only_outside_ascii() {
        assert_eq!(ps1_bytes("a\nb\r\nc\n"), b"a\r\nb\r\nc\r\n".to_vec());
        assert_eq!(ps1_bytes("\u{e9}\n"), "\u{feff}\u{e9}\r\n".as_bytes().to_vec());
    }

    #[test]
    fn offset_map() {
        let src = Source {
            text: "abc\n// --- b\nxyz\n".into(),
            file: "p.nalu".into(),
            embed_root: ".".into(),
            parts: vec![(0, "a.nalu".into(), "abc\n".into()), (13, "b.nalu".into(), "xyz\n".into())],
            extra: Vec::new(),
        };
        let d = Diag::error("E001", Span::new(14, 15), "x");
        let (_, file, local) = src.locate(&d);
        assert_eq!(file, "b.nalu");
        assert_eq!(local.span, Span::new(1, 2));
    }
}
