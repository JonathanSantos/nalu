//! End-to-end tests for Nalu (SPEC-004).
//!
//! - `tests/cases/x.nalu`: compiled with `nalu build` for this machine's Bash target (`macos` on macOS,
//!   `linux` elsewhere) and run with `/bin/bash` in an empty temporary folder; stdout is compared with
//!   `x.out` and the exit code with `x.code` (default 0). Optional: `x.args` (one argument per line), `x.err`
//!   (each non-empty line must appear in stderr; with a leading `!` it must NOT appear in stdout or
//!   stderr), `x.linux.out`/`x.macos.out`/`x.windows.out` (expected output on that target, instead of
//!   `x.out`), `x.env` (`NAME=value` per line), `x.unix` (marker: does not run on PowerShell).
//!   Every generated `.sh` (linux and macos) goes through `bash -n`. The `.ps1` is generated and checked by
//!   the minimal syntax checker `nalu::ps::check_syntax`; if PowerShell is available (`NALU_PWSH`, `pwsh`
//!   or `powershell`), also by the official parser (`tests/ps_syntax.ps1`), and it is run.
//! - `tests/errors/x.nalu`: first line `// error: E0xx` (`check` fails and stderr contains the code)
//!   or `// warning: W0xx` (`check` passes and stderr contains the code).
//! - `examples/*.nalu`: compile for the three targets, `bash -n`, and `nalu test examples` passes.
//! - `tests/pacote/`: `nalu test` with a test that fails on purpose; output equal to `saida.txt`.
//!
//! Each test collects **all** the failures and makes a single `panic!` at the end, with diffs.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(60);

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn nalu() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nalu"))
}

fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().map(|e| e == ext).unwrap_or(false))
        .collect();
    v.sort();
    v
}

fn bash_target() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn bash() -> PathBuf {
    let p = Path::new("/bin/bash");
    if p.is_file() {
        p.to_path_buf()
    } else {
        PathBuf::from("bash")
    }
}

/// The PowerShell used by the tests: `NALU_PWSH`, or `pwsh`/`powershell` on the PATH.
fn powershell() -> Option<String> {
    if let Ok(p) = std::env::var("NALU_PWSH") {
        if !p.is_empty() {
            return Some(p);
        }
    }
    ["pwsh", "powershell"]
        .into_iter()
        .find(|s| {
            Command::new(s)
                .args(["-NoProfile", "-Command", "exit 0"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|o| o.success())
                .unwrap_or(false)
        })
        .map(String::from)
}

static TMP_SEQ: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let n = TMP_SEQ.fetch_add(1, Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("nalu-e2e-{}-{}-{}", std::process::id(), tag, n));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("create temporary folder");
    d
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

/// Runs with a time limit, reading stdout/stderr in threads (so full pipes cannot block it).
fn run(mut cmd: Command) -> Result<Out, String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not run {:?}: {}", cmd, e))?;
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let t1 = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let t2 = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if start.elapsed() > TIMEOUT {
                    let _ = child.kill();
                    timed_out = true;
                    break child.wait().ok();
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(format!("error waiting for the process: {}", e)),
        }
    };
    let stdout = String::from_utf8_lossy(&t1.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&t2.join().unwrap_or_default()).into_owned();
    Ok(Out { code: status.and_then(|s| s.code()), stdout, stderr, timed_out })
}

/// Runs `f` for each item in parallel (up to 8 threads), collecting the failures.
fn par_each<T: Send + Sync + 'static>(items: Vec<T>, f: impl Fn(&T) -> Vec<String> + Send + Sync + 'static) -> Vec<String> {
    let items = Arc::new(items);
    let f = Arc::new(f);
    let next = Arc::new(AtomicUsize::new(0));
    let fails = Arc::new(Mutex::new(Vec::<(usize, String)>::new()));
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    let mut hs = Vec::new();
    for _ in 0..n {
        let (items, f, next, fails) = (items.clone(), f.clone(), next.clone(), fails.clone());
        hs.push(std::thread::spawn(move || loop {
            let i = next.fetch_add(1, Ordering::SeqCst);
            if i >= items.len() {
                break;
            }
            for msg in f(&items[i]) {
                fails.lock().unwrap().push((i, msg));
            }
        }));
    }
    for h in hs {
        let _ = h.join();
    }
    let mut v = fails.lock().unwrap().clone();
    v.sort_by_key(|(i, _)| *i);
    v.into_iter().map(|(_, m)| m).collect()
}

fn report(what: &str, total: usize, fails: Vec<String>) {
    if !fails.is_empty() {
        panic!(
            "\n{} failure(s) in {} ({} item(s) checked):\n\n{}\n",
            fails.len(),
            what,
            total,
            fails.join("\n\n==========\n\n")
        );
    }
}

/// A simple line-by-line diff: differing lines with `-` (expected) and `+` (got).
fn diff(expected: &str, got: &str) -> String {
    let e: Vec<&str> = expected.split('\n').collect();
    let g: Vec<&str> = got.split('\n').collect();
    let mut out = String::new();
    for i in 0..e.len().max(g.len()) {
        match (e.get(i), g.get(i)) {
            (Some(a), Some(b)) if a == b => out.push_str(&format!("  {}\n", a)),
            (a, b) => {
                if let Some(a) = a {
                    out.push_str(&format!("- {}\n", a));
                }
                if let Some(b) = b {
                    out.push_str(&format!("+ {}\n", b));
                }
            }
        }
    }
    out
}

fn first_lines(s: &str, n: usize) -> String {
    let v: Vec<&str> = s.lines().take(n).collect();
    let more = s.lines().count() > n;
    format!("{}{}", v.join("\n"), if more { "\n…" } else { "" })
}

// ---------------------------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------------------------

impl Case {
    fn expected_for(&self, target: &str) -> Option<&String> {
        self.expected_by_target.iter().find(|(t, _)| t == target).map(|(_, o)| o).or(self.expected.as_ref())
    }
}

struct Case {
    file: PathBuf,
    name: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    /// `x.out`, and the per-target variants `x.linux.out`, `x.macos.out`, `x.windows.out`.
    expected: Option<String>,
    expected_by_target: Vec<(String, String)>,
    err_lines: Vec<String>,
    code: i32,
    unix_only: bool,
}

fn load_cases() -> Vec<Case> {
    files(&root().join("tests/cases"), "nalu")
        .into_iter()
        .map(|file| {
            let name = file.file_stem().unwrap().to_string_lossy().into_owned();
            let side = |ext: &str| std::fs::read_to_string(file.with_file_name(format!("{}.{}", name, ext))).ok();
            let args = side("args").map(|s| s.lines().map(String::from).collect()).unwrap_or_default();
            let env = side("env")
                .map(|s| {
                    s.lines()
                        .filter(|l| !l.trim().is_empty())
                        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            let code = side("code").and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            let err_lines = side("err").map(|s| s.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect()).unwrap_or_default();
            Case {
                expected: side("out"),
                expected_by_target: ["linux", "macos", "windows"]
                    .iter()
                    .filter_map(|t| side(&format!("{}.out", t)).map(|o| (t.to_string(), o)))
                    .collect(),
                unix_only: file.with_file_name(format!("{}.unix", name)).exists(),
                file,
                name,
                args,
                env,
                err_lines,
                code,
            }
        })
        .collect()
}

fn build(file: &Path, target: &str, out: &Path) -> Result<(), String> {
    let o = run({
        let mut c = nalu();
        c.arg("build").arg(file).args(["--target", target, "-o"]).arg(out);
        c
    })?;
    if o.code != Some(0) {
        return Err(format!("`nalu build --target {}` failed (code {:?}):\n{}", target, o.code, first_lines(&o.stderr, 30)));
    }
    Ok(())
}

fn bash_n(sh: &Path) -> Result<(), String> {
    let o = run({
        let mut c = Command::new(bash());
        c.arg("-n").arg(sh);
        c
    })?;
    if o.code != Some(0) {
        return Err(format!("`bash -n {}` failed:\n{}", sh.display(), first_lines(&o.stderr, 20)));
    }
    Ok(())
}

fn check_ps1_file(ps1: &Path) -> Result<(), String> {
    let bytes = std::fs::read(ps1).map_err(|e| format!("could not read {}: {}", ps1.display(), e))?;
    // ASCII only and no BOM (`irm … | iex` breaks with a BOM); a BOM only if something is outside ASCII (`native`)
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let body = if bom { &bytes[3..] } else { &bytes[..] };
    if bom == body.is_ascii() {
        return Err(format!("{}: BOM {} with {} content", ps1.display(), if bom { "present" } else { "absent" }, if body.is_ascii() { "ASCII" } else { "non-ASCII" }));
    }
    let text = String::from_utf8_lossy(body).into_owned();
    if text.replace("\r\n", "").contains('\n') {
        return Err(format!("{} has \\n without \\r", ps1.display()));
    }
    let problems = nalu::ps::check_syntax(&text.replace("\r\n", "\n"));
    if !problems.is_empty() {
        return Err(format!("{}: PowerShell syntax checker:\n{}", ps1.display(), problems.join("\n")));
    }
    Ok(())
}

fn compare(case: &Case, label: &str, expected: &str, o: &Out) -> Vec<String> {
    let mut fails = Vec::new();
    let out_ok = o.stdout == expected;
    let code_ok = o.code == Some(case.code);
    // Lines of `x.err`: they must appear in stderr; with a leading `!` they must appear in neither stdout nor stderr.
    let mut missing: Vec<String> = Vec::new();
    for l in &case.err_lines {
        match l.strip_prefix('!') {
            Some(bad) => {
                if o.stderr.contains(bad) || o.stdout.contains(bad) {
                    missing.push(format!("(forbidden) {}", bad));
                }
            }
            None => {
                if !o.stderr.contains(l.as_str()) {
                    missing.push(l.clone());
                }
            }
        }
    }
    if o.timed_out {
        fails.push(format!("{} [{}]: exceeded {:?} and was interrupted", case.name, label, TIMEOUT));
    } else if !out_ok || !code_ok || !missing.is_empty() {
        let mut msg = format!("{} [{}]: {}", case.name, label, case.file.display());
        if !code_ok {
            msg.push_str(&format!("\n  code: expected {}, got {:?}", case.code, o.code));
        }
        if !out_ok {
            msg.push_str(&format!("\n--- stdout diff (- expected, + got) ---\n{}", diff(expected, &o.stdout)));
        }
        for m in missing {
            msg.push_str(&format!("\n  stderr does not match x.err: {:?}", m));
        }
        msg.push_str(&format!("\n--- stderr ---\n{}", first_lines(&o.stderr, 25)));
        fails.push(msg);
    }
    fails
}

#[test]
fn cases_on_bash() {
    if cfg!(windows) {
        return;
    }
    let cases = load_cases();
    let total = cases.len();
    assert!(total > 0, "no cases in tests/cases");
    let fails = par_each(cases, |case| {
        let mut fails = Vec::new();
        let Some(expected) = case.expected_for(bash_target()) else {
            return vec![format!("{}: missing {}.out", case.name, case.name)];
        };
        let dir = temp_dir(&case.name);
        let out = dir.join("dist");
        // bash -n on both Bash targets
        for t in ["linux", "macos"] {
            if let Err(e) = build(&case.file, t, &out) {
                fails.push(format!("{}: {}", case.name, e));
                continue;
            }
            if let Err(e) = bash_n(&out.join(format!("{}.{}.sh", case.name, t))) {
                fails.push(format!("{}: {}", case.name, e));
            }
        }
        if !fails.is_empty() {
            let _ = std::fs::remove_dir_all(&dir);
            return fails;
        }
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let sh = out.join(format!("{}.{}.sh", case.name, bash_target()));
        let mut c = Command::new(bash());
        c.arg(&sh).args(&case.args).current_dir(&work);
        for (k, v) in &case.env {
            c.env(k, v);
        }
        match run(c) {
            Ok(o) => fails.extend(compare(case, bash_target(), expected, &o)),
            Err(e) => fails.push(format!("{}: {}", case.name, e)),
        }
        let _ = std::fs::remove_dir_all(&dir);
        fails
    });
    report("tests/cases (Bash)", total, fails);
}

/// A Bash 4+ to run the `linux` target outside Linux: `NALU_BASH4`, or the Homebrew/`/usr/local` one.
fn bash4() -> Option<PathBuf> {
    let cands: Vec<PathBuf> = std::env::var("NALU_BASH4").ok().map(PathBuf::from).into_iter()
        .chain(["/opt/homebrew/bin/bash", "/usr/local/bin/bash"].iter().map(PathBuf::from))
        .collect();
    cands.into_iter().find(|b| {
        Command::new(b).args(["-c", "(( BASH_VERSINFO[0] * 100 + BASH_VERSINFO[1] >= 403 ))"]).status().map(|s| s.success()).unwrap_or(false)
    })
}

/// On Linux, `cases_on_bash` already runs the `linux` target. Elsewhere (macOS), with Bash 4.3+ installed,
/// it also runs the `linux` script: this covers the code specific to that target (`declare -A`, `${x,,}`,
/// `wait -n`, lists by reference). The external tools are the system's (BSD on macOS), not GNU.
#[test]
fn cases_on_bash_linux_target() {
    if cfg!(target_os = "linux") || cfg!(windows) {
        return;
    }
    let Some(b4) = bash4() else {
        eprintln!("warning: no Bash 4.3+ (set NALU_BASH4): the linux target was only validated by `bash -n`");
        return;
    };
    let cases = load_cases();
    let total = cases.len();
    let fails = par_each(cases, move |case| {
        let mut fails = Vec::new();
        let Some(expected) = case.expected_for("linux") else {
            return vec![format!("{}: missing {}.out", case.name, case.name)];
        };
        let dir = temp_dir(&format!("{}-linux", case.name));
        let out = dir.join("dist");
        if let Err(e) = build(&case.file, "linux", &out) {
            let _ = std::fs::remove_dir_all(&dir);
            return vec![format!("{}: {}", case.name, e)];
        }
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let mut c = Command::new(&b4);
        c.arg(out.join(format!("{}.linux.sh", case.name))).args(&case.args).current_dir(&work);
        for (k, v) in &case.env {
            c.env(k, v);
        }
        match run(c) {
            Ok(o) => fails.extend(compare(case, "linux", expected, &o)),
            Err(e) => fails.push(format!("{}: {}", case.name, e)),
        }
        let _ = std::fs::remove_dir_all(&dir);
        fails
    });
    report("tests/cases (Bash 4+, linux target)", total, fails);
}

#[test]
fn cases_on_powershell() {
    let cases = load_cases();
    let total = cases.len();
    let pwsh = powershell();
    let fails = par_each(cases, move |case| {
        let mut fails = Vec::new();
        let dir = temp_dir(&format!("ps-{}", case.name));
        let out = dir.join("dist");
        if let Err(e) = build(&case.file, "windows", &out) {
            let _ = std::fs::remove_dir_all(&dir);
            return vec![format!("{}: {}", case.name, e)];
        }
        let ps1 = out.join(format!("{}.windows.ps1", case.name));
        if let Err(e) = check_ps1_file(&ps1) {
            fails.push(format!("{}: {}", case.name, e));
        }
        if let (Some(ps), true) = (&pwsh, fails.is_empty()) {
            let syn = root().join("tests/ps_syntax.ps1");
            match run({
                let mut c = Command::new(ps);
                c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&syn).arg(&ps1);
                c
            }) {
                Ok(o) if o.code == Some(0) => {}
                Ok(o) => fails.push(format!("{}: tests/ps_syntax.ps1 rejected the script:\n{}{}", case.name, o.stdout, o.stderr)),
                Err(e) => fails.push(format!("{}: {}", case.name, e)),
            }
            let expected = case.expected_for("windows");
            if let (false, Some(expected), true) = (case.unix_only, expected, fails.is_empty()) {
                let work = dir.join("work");
                std::fs::create_dir_all(&work).unwrap();
                let mut c = Command::new(ps);
                c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&ps1).args(&case.args).current_dir(&work);
                for (k, v) in &case.env {
                    c.env(k, v);
                }
                match run(c) {
                    Ok(o) => {
                        let o = Out { stdout: o.stdout.replace("\r\n", "\n"), ..o };
                        fails.extend(compare(case, "powershell", expected, &o))
                    }
                    Err(e) => fails.push(format!("{}: {}", case.name, e)),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        fails
    });
    if powershell().is_none() {
        eprintln!("warning: no PowerShell (set NALU_PWSH): the .ps1 files were only generated and checked by the minimal checker");
    }
    report("tests/cases (PowerShell)", total, fails);
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

#[test]
fn compile_errors() {
    let list = files(&root().join("tests/errors"), "nalu");
    let total = list.len();
    let fails = par_each(list, |file| {
        let name = file.display().to_string();
        let text = match std::fs::read_to_string(file) {
            Ok(t) => t,
            Err(e) => return vec![format!("{}: could not read: {}", name, e)],
        };
        let first = text.lines().next().unwrap_or("").trim().to_string();
        let (want_error, code) = if let Some(c) = first.strip_prefix("// error:") {
            (true, c.trim().to_string())
        } else if let Some(c) = first.strip_prefix("// warning:") {
            (false, c.trim().to_string())
        } else {
            return vec![format!("{}: the first line must be `// error: E0xx` or `// warning: W0xx`, found {:?}", name, first)];
        };
        let o = match run({
            let mut c = nalu();
            c.arg("check").arg(file);
            c
        }) {
            Ok(o) => o,
            Err(e) => return vec![format!("{}: {}", name, e)],
        };
        let mut fails = Vec::new();
        if want_error && o.code != Some(1) {
            fails.push(format!("{}: `nalu check` should fail (code 1), got {:?}", name, o.code));
        }
        if !want_error && o.code != Some(0) {
            fails.push(format!("{}: `nalu check` should pass (warning only), got {:?}", name, o.code));
        }
        if !o.stderr.contains(&code) {
            fails.push(format!("{}: stderr does not contain `{}`", name, code));
        }
        if !fails.is_empty() {
            fails.push(format!("--- stderr of {} ---\n{}", name, first_lines(&o.stderr, 15)));
        }
        fails
    });
    report("tests/errors", total, fails);
}

// ---------------------------------------------------------------------------------------------
// Examples and test package
// ---------------------------------------------------------------------------------------------

#[test]
fn examples_compile_for_all_three_targets() {
    // `x_test.nalu` only has `test` blocks that use the neighboring file: it is covered by `nalu test examples`.
    let list: Vec<PathBuf> = files(&root().join("examples"), "nalu")
        .into_iter()
        .filter(|f| !f.file_stem().unwrap().to_string_lossy().ends_with("_test"))
        .collect();
    let total = list.len();
    let fails = par_each(list, |file| {
        let name = file.file_stem().unwrap().to_string_lossy().into_owned();
        let dir = temp_dir(&format!("ex-{}", name));
        let mut fails = Vec::new();
        match build(file, "linux,macos,windows", &dir) {
            Err(e) => fails.push(format!("{}: {}", name, e)),
            Ok(()) => {
                for t in ["linux", "macos"] {
                    if let Err(e) = bash_n(&dir.join(format!("{}.{}.sh", name, t))) {
                        fails.push(format!("{}: {}", name, e));
                    }
                }
                let ps1 = dir.join(format!("{}.windows.ps1", name));
                if let Err(e) = check_ps1_file(&ps1) {
                    fails.push(format!("{}: {}", name, e));
                }
                if let Some(ps) = powershell() {
                    let syn = root().join("tests/ps_syntax.ps1");
                    match run({
                        let mut c = Command::new(ps);
                        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&syn).arg(&ps1);
                        c
                    }) {
                        Ok(o) if o.code == Some(0) => {}
                        Ok(o) => fails.push(format!("{}: tests/ps_syntax.ps1 rejected it:\n{}{}", name, o.stdout, o.stderr)),
                        Err(e) => fails.push(format!("{}: {}", name, e)),
                    }
                }
                let cmd = std::fs::read_to_string(dir.join(format!("{}.windows.cmd", name))).unwrap_or_default();
                if cmd != nalu::ps::launcher(&name) {
                    fails.push(format!("{}: .cmd differs from the contract:\n{:?}", name, cmd));
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        fails
    });
    report("examples (build)", total, fails);
}

#[test]
fn nalu_test_examples_passes() {
    let o = run({
        let mut c = nalu();
        c.arg("test").arg(root().join("examples"));
        c
    })
    .unwrap();
    if o.code != Some(0) {
        panic!("`nalu test examples` should pass (code 0), got {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}", o.code, o.stdout, first_lines(&o.stderr, 40));
    }
    assert!(o.stdout.contains("ok "), "the output lists no tests: {}", o.stdout);
    if powershell().is_some() {
        let o = run({
            let mut c = nalu();
            c.arg("test").arg(root().join("examples")).args(["--target", "windows"]);
            c
        })
        .unwrap();
        if o.code != Some(0) {
            panic!("`nalu test examples --target windows` should pass, got {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}", o.code, o.stdout, first_lines(&o.stderr, 40));
        }
    }
}

#[test]
fn nalu_test_package_with_failure() {
    let dir = root().join("tests/pacote");
    let expected = std::fs::read_to_string(dir.join("saida.txt")).expect("tests/pacote/saida.txt");
    let mut fails = Vec::new();
    let mut targets = vec![bash_target()];
    if powershell().is_some() {
        targets.push("windows");
    }
    for t in &targets {
        let o = run({
            let mut c = nalu();
            c.arg("test").arg(&dir).args(["--target", t]);
            c
        })
        .unwrap();
        let out = o.stdout.replace("\r\n", "\n");
        let mut f = Vec::new();
        if o.code != Some(1) {
            f.push(format!("[{}] `nalu test tests/pacote` should exit with code 1 (a test fails on purpose), got {:?}", t, o.code));
        }
        if out != expected {
            f.push(format!("[{}] --- diff (- expected, + got) ---\n{}", t, diff(&expected, &out)));
        }
        if !f.is_empty() {
            f.push(format!("[{}] --- stderr ---\n{}", t, first_lines(&o.stderr, 30)));
        }
        fails.extend(f);
    }
    report("tests/pacote", targets.len(), fails);
}

// ---------------------------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------------------------

fn nalu_out(args: &[&str]) -> Out {
    run({
        let mut c = nalu();
        c.args(args);
        c
    })
    .unwrap()
}

/// The Copilot instructions (extension and repository) are the same file, and the example script in them
/// compiles without warnings and is already formatted.
#[test]
fn copilot_instructions_match() {
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ext = std::fs::read_to_string(root_dir.join("editors/vscode/copilot/nalu.instructions.md")).unwrap();
    let repo = std::fs::read_to_string(root_dir.join(".github/instructions/nalu.instructions.md")).unwrap();
    assert_eq!(ext, repo, "copy editors/vscode/copilot/nalu.instructions.md to .github/instructions/");
    let start = ext.find("```nalu\n").expect("a ```nalu block") + "```nalu\n".len();
    let end = start + ext[start..].find("```").unwrap();
    let code = &ext[start..end];
    let c = nalu::compile(code, "instructions.nalu", root_dir, nalu::Target::Linux);
    assert!(c.diags.is_empty(), "{:?}", c.diags);
    assert_eq!(nalu::fmt::format(code).unwrap(), code, "the example must be formatted");
}

/// The documents people read: README, docs (outside `history/`), examples, extension, llms.txt.
fn documents() -> Vec<PathBuf> {
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut v: Vec<PathBuf> = ["README.md", "README.pt-BR.md", "CHANGELOG.md", "llms.txt", "examples/README.md", "editors/vscode/README.md", "docs/history/README.md"]
        .iter()
        .map(|f| root_dir.join(f))
        .collect();
    let mut docs: Vec<PathBuf> = std::fs::read_dir(root_dir.join("docs")).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().map(|x| x == "md").unwrap_or(false)).collect();
    docs.sort();
    v.extend(docs);
    v.into_iter().filter(|p| p.is_file()).collect()
}

/// Every ` ```nalu ` block in the documentation is a complete program: it compiles without errors and is already formatted.
#[test]
fn documentation_examples_compile() {
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut fails = Vec::new();
    let mut n = 0;
    for f in documents() {
        let text = std::fs::read_to_string(&f).unwrap();
        let mut rest = text.as_str();
        let mut offset = 0;
        while let Some(i) = rest.find("```nalu\n") {
            let ini = i + "```nalu\n".len();
            let Some(len) = rest[ini..].find("```") else { break };
            let code = &rest[ini..ini + len];
            let line = text[..offset + i].lines().count() + 1;
            n += 1;
            let c = nalu::compile(code, "block.nalu", root_dir, nalu::Target::Linux);
            let errs: Vec<String> = c.diags.iter().filter(|d| d.is_error()).map(|d| d.render(code, "block.nalu")).collect();
            if !errs.is_empty() {
                fails.push(format!("{}:{}: does not compile:\n{}", f.display(), line, errs.join("\n")));
            }
            match nalu::fmt::format(code) {
                Ok(fmt) if fmt != code => fails.push(format!("{}:{}: is not formatted (run `nalu fmt`)", f.display(), line)),
                _ => {}
            }
            offset += ini + len + 3;
            rest = &text[offset..];
        }
    }
    assert!(n >= 20, "only {} ```nalu blocks in the documentation", n);
    report("```nalu blocks in the documentation", n, fails);
}

/// Relative links in the documents point to files that exist.
#[test]
fn documentation_links_exist() {
    let mut fails = Vec::new();
    let mut n = 0;
    for f in documents() {
        // without the code blocks: `[string]('' + x)` is not a link
        let raw = std::fs::read_to_string(&f).unwrap();
        let text: String = raw.split("```").enumerate().filter(|(i, _)| i % 2 == 0).map(|(_, t)| t).collect::<Vec<_>>().join("\n");
        let dir = f.parent().unwrap().to_path_buf();
        let mut targets: Vec<String> = Vec::new();
        for (open, close) in [("](", ')'), ("href=\"", '"'), ("srcset=\"", '"'), ("src=\"", '"')] {
            let mut rest = text.as_str();
            while let Some(i) = rest.find(open) {
                let r = &rest[i + open.len()..];
                if let Some(j) = r.find(close) {
                    targets.push(r[..j].to_string());
                }
                rest = &rest[i + open.len()..];
            }
        }
        for a in targets {
            let a = a.split_whitespace().next().unwrap_or("");
            if a.is_empty() || a.starts_with("http") || a.starts_with('#') || a.starts_with("mailto:") || a.contains('{') || a.contains('`') {
                continue;
            }
            let target_path = a.split('#').next().unwrap_or("");
            n += 1;
            if !dir.join(target_path).exists() {
                fails.push(format!("{}: broken link: {}", f.display(), a));
            }
        }
    }
    report("documentation links", n, fails);
}

/// `docs/stdlib.md` is generated by the compiler (`nalu doc --all > docs/stdlib.md`) and must not fall behind.
#[test]
fn stdlib_reference_is_up_to_date() {
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc = std::fs::read_to_string(root_dir.join("docs/stdlib.md")).unwrap_or_default();
    assert!(doc == nalu::ide::stdlib_reference(), "docs/stdlib.md is out of date: run `nalu doc --all > docs/stdlib.md`");
}

#[test]
fn cli_help_version_and_misuse() {
    let mut fails = Vec::new();
    let h = nalu_out(&["--help"]);
    for sub in ["run", "build", "check", "emit", "test", "fmt", "doc", "ide"] {
        if !h.stdout.contains(&format!("nalu {}", sub)) {
            fails.push(format!("--help does not list `{}`", sub));
        }
    }
    if h.code != Some(0) {
        fails.push(format!("--help: code {:?}", h.code));
    }
    let v = nalu_out(&["--version"]);
    if !v.stdout.starts_with("nalu ") || v.code != Some(0) {
        fails.push(format!("--version: {:?} {:?}", v.stdout, v.code));
    }
    let foo = nalu_out(&["foo"]);
    if foo.code != Some(2) || !foo.stderr.contains("nalu --help") {
        fails.push(format!("`nalu foo` should exit with code 2 and `use nalu --help`: {:?} {:?}", foo.code, foo.stderr));
    }
    let flag = nalu_out(&["build", "x.nalu", "--nada"]);
    if flag.code != Some(2) {
        fails.push(format!("an unknown flag should exit with code 2: {:?}", flag.code));
    }
    let none = nalu_out(&["check", "nao-existe-mesmo.nalu"]);
    if none.code != Some(1) || !none.stderr.contains("nao-existe-mesmo.nalu") {
        fails.push(format!("check of a missing file should exit with code 1 and name the file: {:?} {:?}", none.code, none.stderr));
    }
    report("CLI", 5, fails);
}

#[test]
fn cli_build_emits_four_files() {
    let dir = temp_dir("build-hello");
    let file = root().join("examples/hello.nalu");
    let o = nalu_out(&["build", file.to_str().unwrap(), "-o", dir.to_str().unwrap()]);
    let mut fails = Vec::new();
    if o.code != Some(0) {
        fails.push(format!("`nalu build examples/hello.nalu` failed ({:?}):\n{}", o.code, first_lines(&o.stderr, 30)));
    } else {
        for f in ["hello.linux.sh", "hello.macos.sh", "hello.windows.ps1", "hello.windows.cmd"] {
            if !dir.join(f).is_file() {
                fails.push(format!("missing {}", f));
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in ["hello.linux.sh", "hello.macos.sh"] {
                let mode = std::fs::metadata(dir.join(f)).map(|m| m.permissions().mode()).unwrap_or(0);
                if mode & 0o111 == 0 {
                    fails.push(format!("{} is not executable ({:o})", f, mode));
                }
            }
        }
        if let Err(e) = check_ps1_file(&dir.join("hello.windows.ps1")) {
            fails.push(e);
        }
        let cmd = std::fs::read_to_string(dir.join("hello.windows.cmd")).unwrap_or_default();
        let want = "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0hello.windows.ps1\" %*\r\nexit /b %ERRORLEVEL%\r\n";
        if cmd != want {
            fails.push(format!(".cmd: {:?}", cmd));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    report("nalu build", 1, fails);
}

#[test]
fn cli_run_forwards_arguments_and_exit_code() {
    if cfg!(windows) {
        return;
    }
    let file = root().join("examples/hello.nalu");
    let o = nalu_out(&["run", file.to_str().unwrap(), "--", "--name", "Ana", "--times", "2"]);
    let mut fails = Vec::new();
    if o.code != Some(0) {
        fails.push(format!("`nalu run examples/hello.nalu -- --name Ana --times 2` gave {:?}:\n{}", o.code, first_lines(&o.stderr, 30)));
    }
    for l in ["hello, Ana! (1/2)", "hello, Ana! (2/2)"] {
        if !o.stdout.contains(l) {
            fails.push(format!("missing line {:?} in the output:\n{}", l, o.stdout));
        }
    }
    report("nalu run", 1, fails);
}

#[test]
fn cli_check_json() {
    // A file that is invalid on purpose: `--json` prints only JSON on stdout.
    let dir = temp_dir("json");
    let f = dir.join("bad.nalu");
    std::fs::File::create(&f).unwrap().write_all(b"let x = nao_existe\nprint(x)\n").unwrap();
    let o = nalu_out(&["check", f.to_str().unwrap(), "--json"]);
    let mut fails = Vec::new();
    if o.code != Some(1) {
        fails.push(format!("check --json of an invalid file should exit with 1, got {:?}", o.code));
    }
    let t = o.stdout.trim();
    if !(t.starts_with('[') && t.ends_with(']') && t.contains("\"code\":\"E")) {
        fails.push(format!("stdout is not the expected JSON list: {:?}", o.stdout));
    }
    let _ = std::fs::remove_dir_all(&dir);
    report("nalu check --json", 1, fails);
}

#[test]
fn cli_check_test_package() {
    // `x_test.nalu` is checked together with `x.nalu` (no false E001). With --stdin, as in an editor, only
    // the diagnostics and fallible points of the open file appear, on its own correct line.
    let dir = temp_dir("pacote-check");
    std::fs::write(dir.join("calc.nalu"), "fn dobro(n: int) -> int {\n    return n * 2\n}\nlet v = \"x\".to_int()\nprint(v)\n").unwrap();
    let t = dir.join("calc_test.nalu");
    std::fs::write(&t, "test \"dobro\" {\n    assert dobro(2) == 4\n}\n").unwrap();
    let mut fails = Vec::new();
    let o = nalu_out(&["check", t.to_str().unwrap()]);
    if o.code != Some(0) {
        fails.push(format!("check of the test file should pass (package), got {:?}:\n{}", o.code, o.stderr));
    }
    let mut c = nalu();
    c.args(["check", "--stdin", t.to_str().unwrap(), "--json", "--explain"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"test \"dobro\" {\n    assert dobro(2) == 4\n}\nlet quebrado: int = \"x\"\n").unwrap();
    let out = child.wait_with_output().unwrap();
    let so = String::from_utf8_lossy(&out.stdout).into_owned();
    if !so.contains("\"line\":4,") || !so.contains("\"code\":\"E010\"") {
        fails.push(format!("the error should be on line 4 of the test file: {}", so));
    }
    if so.contains("calc.nalu\"") {
        fails.push(format!("with --stdin, nothing from calc.nalu should appear: {}", so));
    }
    let _ = std::fs::remove_dir_all(&dir);
    report("nalu check of a test package", 2, fails);
}

#[test]
fn use_splits_a_script_across_files() {
    // tests/use: main.nalu uses lib/services.nalu and lib/text.nalu, which uses text.nalu again (loaded once).
    let dir = root().join("tests/use");
    let main = dir.join("main.nalu");
    let out = temp_dir("use");
    let mut fails = Vec::new();
    let o = nalu_out(&["build", main.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    if o.code != Some(0) {
        fails.push(format!("`nalu build tests/use/main.nalu` gave {:?}:\n{}", o.code, first_lines(&o.stderr, 30)));
    }
    let want = "HELLO, ANA!\nhttp://localhost:8080 API!\nproduction\n";
    let mut runs: Vec<(String, Command)> = Vec::new();
    if !cfg!(windows) {
        let mut c = Command::new(bash());
        c.arg(out.join(format!("main.{}.sh", bash_target()))).args(["--name", "Ana"]);
        runs.push(("bash".into(), c));
    }
    if let Some(ps) = powershell() {
        let mut c = Command::new(ps);
        c.args(["-NoProfile", "-File"]).arg(out.join("main.windows.ps1")).args(["--name", "Ana"]);
        runs.push(("powershell".into(), c));
    }
    for (label, cmd) in runs {
        match run(cmd) {
            Ok(r) if r.code == Some(0) && r.stdout.replace("\r\n", "\n") == want => {}
            Ok(r) => fails.push(format!("[{}] code {:?}, stdout:\n{}\nstderr:\n{}", label, r.code, r.stdout, first_lines(&r.stderr, 20))),
            Err(e) => fails.push(format!("[{}] {}", label, e)),
        }
    }
    // problems with `use` are reported in the right file, like any diagnostic
    for (file, place) in [("missing.nalu", "missing.nalu:2:1"), ("code_in_lib.nalu", "with_code.nalu:6:1")] {
        let o = nalu_out(&["check", dir.join(file).to_str().unwrap()]);
        if o.code != Some(1) || !o.stderr.contains("E055") || !o.stderr.contains(place) {
            fails.push(format!("`nalu check tests/use/{}` should fail with E055 at {}: {:?}\n{}", file, place, o.code, first_lines(&o.stderr, 10)));
        }
    }
    // the editor follows names into the used files
    let o = nalu_out(&["ide", "definition", main.to_str().unwrap(), "--at", "9:8"]);
    if !o.stdout.contains("lib/text.nalu") {
        fails.push(format!("go to definition of `shout` should open lib/text.nalu: {}", o.stdout));
    }
    report("use", 4, fails);
}
