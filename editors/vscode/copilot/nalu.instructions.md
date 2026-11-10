---
description: How to write Nalu (.nalu) scripts — syntax, failure handling with `catch`, `$` commands, and the standard library.
applyTo: "**/*.nalu"
---

# Writing Nalu

Nalu is a small typed language for setup, build, deploy and install scripts. A `.nalu` file compiles to Bash (Linux, macOS) and PowerShell (Windows). It is **not** Rust, Swift, Go or JavaScript: do not use their syntax or libraries. Nalu has one way to do each thing; the compiler rejects the others and names the right one.

## Check your work

When the Nalu tools are available, use them instead of guessing:

- `#naluDocs` with names like `fs`, `fs.copy`, `str`, `str.split`, `path`, `err` or `catch` returns the exact signature and behavior. Look up every standard library call you are not sure about.
- `#naluCheck` compiles the code with the real compiler. Run it on every file you write or change, and fix every error before answering.
- `#naluFormat` returns the code in the house style.

Without the tools, run `nalu check file.nalu`, `nalu fmt file.nalu` and `nalu doc fs.copy` in a terminal.

## Rules models get wrong

- **Failures propagate.** Anything that can fail (a `$` command, `fs.read`, `env.get`, `.to_int()`, `xs[i]`, `m[k]`, `.first()`, any function of yours that calls one of these) stops the script, or makes the enclosing function fail, unless it is handled with `catch`. There is no `?`, no `throws`, no `null`, no exceptions to declare.
- **`catch` handles a failure:**
  - `let text = fs.read(p) catch ""` — a default value (it can use `err`: `catch err.code`, `catch "failed: {err}"`)
  - `let port = env.get("PORT") catch fail "set PORT"` — fail with a better message (`{err}` is available)
  - `$ git pull catch ignore` — keep going
  - `catch { … }` — a block that can use `err` (`err.message`, `err.code`); in a value position its last line is the value
  - `try { … } catch …` — one handler for several lines
- **Logic:** `||`, `&&` and `!`. There is no `or`, `and` or `not`.
- **Commands.** A line that starts with `$ ` runs a program: `$ docker build -t {image}:{tag} .`. Words are literal (no globbing, no `$VAR`); `{x}` is exactly one argument, and a list `{xs}` alone becomes several. `$(git rev-parse --short HEAD)` captures stdout as `str`. `secret | $ docker login --password-stdin` sends a value to stdin. `if $ git diff --quiet { … }` uses the exit code as a condition. An unquoted ` catch ` ends the command.
- **A variable for one program** goes before it, as in a shell: `$ CGO_ENABLED=0 go build`, `$ GH_TOKEN={token} gh release create v1`. It reaches only that program; a `secret` is fine there. For the rest of the script use `env.set`.
- **Text.** `"hello {name}"` interpolates any expression in `{ }`; `{{` and `}}` are literal braces. Interpolation is the only way to turn a value into text (there is no `.to_str()`). `'raw'` has no interpolation or escapes (use it for fixed JSON). `"""…"""` is multi-line.
- **Declarations.** `let` is immutable, `var` is mutable; there is no `const`, `++` or `--`. `fn name(a: int, b: str = "x") -> str { … }`; no `function`, `def` or `func`. No semicolons. Names are snake_case, types PascalCase.
- **Types are written like Go:** `[]str`, `[]int`, `[][]str`, `map[str]int`, `map[str][]str`. A struct field can be another struct. Nested values change in place: `grid[i][j] = v`, `svcs[0].port = 80`, `groups[k].push(x)`, `let x = rows[0].pop()`.
- **Expressions.** `if` and `match` return values: `let r = if ok { 1 } else { 2 }`. There is no `? :`. A `match` has one arm per line, tried in order (`"a" | "b" => …`, `200..=299 => …`, `n if n >= 500 => "error {n}"`, `Service { port: 443 } => …`, `_ => …`); a name takes the value, `if` adds a guard; inside it, enum cases are written without the type name, and a `match` on an enum must cover every case or end with `_`.
- **Loops.** `for x in xs`, `for i, x in xs`, `for k, v in m`, `for n in 0..10` (excludes 10), `0..=10` (includes it), `while cond { }`.
- **One way each.** Maps: `m[k]` reads (can fail), `m[k] = v` writes. Empty checks: `s == ""`, `xs.len() == 0`. Paths: `.name`, `.stem`, `.ext`, `.parent` only look at the text; anything on disk is `fs.exists(p)`, `fs.is_dir(p)`, `fs.size(p)`, `fs.modified(p)`; join with `a / "b"`. Failing on purpose: `fail "message"` or `fail "message", code: 3`.
- **No imports.** Every standard library module is always available. To split a script, `use "lib.nalu"` at the start of a file brings in that file's `fn`, `struct`, `enum` and `test` declarations. Unused variables are a warning; prefix them with `_` on purpose.

## Types

`str`, `int` (64-bit; `/` truncates), `bool`, `path` (`dirs.home() / "x"` joins with the system separator), `duration` (`500ms`, `30s`, `1m30s`, `2h`, `7d`), `secret` (never printed or passed as an argument), `[]T` (items: scalars, structs or lists of scalars), `map[K]V` (`{ "a": 1 }`; values: scalars or lists of scalars), `struct` (fields can be structs or lists of structs), `enum`. There is no float.

## A script

```nalu
/// shown in --help
param stage: Stage = Stage.dev
param version: str
param token: secret  // also read from NALU_TOKEN

enum Stage { dev, prod }

struct Service {
    name: str
    port: int

    fn url() -> str {
        return "http://localhost:{self.port}"
    }
}

let services = [Service { name: "api", port: 8080 }]
log.info("deploying {version} to {stage}")
let tmp = fs.temp_dir()
defer fs.rm(tmp)  // runs at exit, even after a failure or Ctrl+C
token | $ docker login registry.example.com --password-stdin
parallel for s in services {
    $ docker push {s.name}:{version}
}
retry(10, every: 5s) {
    if http.status(services[0].url()) != 200 {
        fail "not up yet"
    }
}
let host = match stage {
    dev  => "dev.example.com"
    prod => "example.com"
}
if os == "windows" {
    $ explorer {tmp} catch ignore
}
log.info("live at https://{host}")
```

Every generated script takes `--help`, `--dry-run` (commands and writes are only shown) and `--verbose`. `os` is `"linux"`, `"macos"` or `"windows"` at compile time; `dry_run` and `args: []str` are always defined. Tests go in `x_test.nalu`: `test "name" { … }` blocks with `assert cond` and `assert_fails(call())` on their own lines, run with `nalu test`.

## Standard library (main calls; "!" = can fail)

- built-ins: `print(a, …)`, `fmt(template, a, …)` (for templates that come from data), `exit(code)`, `secret(s)`, `embed("file")`
- `env`: `get(name)!`, `has(name)`, `set(name, value)`, `unset(name)`
- `fs`: `exists`, `is_dir`, `is_file`, `size!`, `modified!`, `read(p)!`, `write(p, text)!`, `append(p, text)!`, `rm(p)!`, `mkdir(p)!`, `copy(from, to)!`, `move(from, to)!`, `ls(dir)!`, `walk(dir)!`, `glob(pattern)`, `temp_dir()!`, `temp_file()!`, `replace(p, from, to)!`, `chmod(p, mode)!`
- `path` values: `.name`, `.stem`, `.ext`, `.parent` (no parentheses); `a / "b"`; `path.abs(p)`
- `dirs`: `home()`, `temp()`, `cwd()`, `config(app)`, `cache(app)`, `data(app)` · `sys`: `arch()`, `user()`, `is_admin()`, `is_ci()`, `cpus()`, `cd(dir)!` · `proc`: `exists(cmd)`, `which(cmd)!`
- `time`: `now()`, `since(t)`, `format(fmt, t)`, `sleep(d)` · `log`: `info`, `warn`, `error`, `debug` · `ui`: `confirm(q)`, `ask(q, fallback)`, `password(q)!`, `choose(q, options)!`
- `http`: `get(url, timeout, headers, bearer)!`, `post(url, body, …)!`, `request("PUT", url, body, …)!` (POST, PUT, PATCH, DELETE), `status(url, …)!`, `download(url, to, …)!` · `json`: `get(text, "a.b[0]")!`, `get_list`, `keys`, `has`, `quote` · `hash`: `sha256(text)`, `sha256_file(p)!` · `archive`: `tar_create`, `tar_extract`, `zip_extract`
- `regex` (the pattern is fixed text in single quotes, checked by the compiler): `test(text, pattern)`, `find(text, pattern)!`, `find_all`, `groups(text, pattern)!`, `replace(text, pattern, new)`; `find`/`find_all`/`groups`/`replace` need a pattern with one way to match (`[^=]*=`, not `.*=`) · `encoding`: `base64_encode(text)`, `base64_decode(text)!`
- `str`: `len()`, `trim()`, `upper()`, `lower()`, `contains(s)`, `starts_with(s)`, `split(sep)`, `lines()`, `replace(a, b)`, `slice(a, b)`, `to_int()!`, …
- lists: `len()`, `push(x)`, `pop()!`, `filter(x => …)`, `map(x => …)`, `find(…)!`, `first()!`, `last()!`, `contains(x)`, `join(sep)`, `sort()`, `sort_by(…)`, `unique()`, `slice(a, b)`, `concat(ys)` (lists are not added with `+`)
- maps: `m[k]!`, `m[k] = v`, `has(k)`, `remove(k)`, `keys()`, `values()`, `len()`
- `pkg`: `manager()` (`apt-get`, `brew`, `winget`… or `""`), `install(names)!` · `semver`: `compare(a, b)!` → -1, 0, 1 (`semver.compare(have, "2.30") >= 0`)
