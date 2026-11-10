# Changelog

## Unreleased

### Fixed

- Alpine (musl): `encoding.base64_decode` accepted bytes that aren't UTF-8, because musl's `iconv` drops them and still succeeds. The decoded bytes are now compared after the round trip.
- The test suite passes on Linux, Alpine and Windows, not only on macOS: the Bash backend tests run the target of the machine they are on, and are skipped on Windows.

## 1.1.0 — 2026-11-09

Everything that compiled with 1.0 compiles with 1.1 and does the same thing, with one exception listed under *Fixed*: a `match` arm placed after `_` used to run before `_`, and is now an error, because it could never be reached as written.

### The language

- **`regex`**: `test`, `find`, `find_all`, `groups` and `replace`, in one regex syntax (`.`, classes with ASCII ranges, `\d \w \s`, greedy `* + ? {n,m}`, groups, `|`, `^`/`$`, `(?i)`). The pattern is fixed text in the call; the compiler checks it and translates it to POSIX ERE for Bash and to .NET for PowerShell, so it matches the same text on every target. Functions that return matched text accept only patterns with a single way to match, and the error says what to write instead (`[^=]*` rather than `.*`). New error code `E054`.
- **`encoding`**: `base64_encode` and `base64_decode` (whitespace, the URL-safe alphabet and missing padding accepted; fails on anything that isn't Base64 of UTF-8 text).
- **`match`**: guards (`n if n >= 500 => …`), a name that takes the value (`n => "status {n}"`), and struct patterns (`Service { port: 443, name: n } => …`). Arms are tried in order. On an enum a bare name is still a case, so a typo is an error, not a new variable.
- **Nested collections**: `[][]T` and `map[K][]T` (with `T` a scalar), including `.map()` returning lists, `contains`/`index_of`/`unique` on whole items, and loops over items.
- **Structs inside structs and lists of structs as fields**: a field can be another struct (`port: Port`) or a list of them (`services: []Service`); `a.b.c` and `cfg.services[0].port` read and write them.
- **Changing nested values in place**: `grid[i][j] = v`, `services[0].port = 80`, `config.server.port = 8080`, `cfg.services.push(s)`, and `groups[k].push(x)` or `let x = rows[0].pop()` (as a statement or the whole value of `let`, `=` or `return`).
- **Several files**: `use "lib.nalu"` at the start of a file brings in the `fn`, `struct`, `enum` and `test` declarations of another file (recursively, each file once). `nalu build` still writes one self-contained script per target; the editor follows names into used files. New error code `E055`.
- `code`, `max` and `every` are no longer reserved: they are ordinary names (named arguments are recognized by their `name:` label).
- **`parallel` on Windows runs at the same time**, in a runspace pool, with the same results, errors and isolation as the Bash background jobs. Tasks that call `sys.cd` or `env.set` run one at a time there.
- A program used only inside `if proc.exists("x") { … }` is optional: the script doesn't require it at startup and `W010` doesn't warn about it.
- `param` accepts `[]path`. A list of paths passed to a program has its leading `~` expanded, like a single path.
- `$ cmd catch { $ other }` works on one line, and the `||` hint points to it.

### Fixed

- A `match` arm after `_` (or after any arm that matches everything) is `E031`; before, it silently ran before `_`.
- Bash: a `parallel for` that returns structs gave an empty list.
- Bash 5.1+: `max` was ignored in every `parallel` after the first one.
- macOS: `parallel(max: 0)` never finished; a `max` below 1 now counts as 1.
- Bash: a `path` like `~user/x` passed to a program became `$HOME` + `user/x`; `~` and `~/…` are expanded, anything else is left alone, as on Windows.
- Bash: `--verbose` now shows `warning: defer failed: …` like PowerShell.
- `nalu fmt` no longer adds a space in `regex.test(…)` (a keyword used as a member name).

### Inside

- The code comments, test names and test labels are in English, and the test corpus files have English names.
- New IR passes (`src/nest.rs`) keep both backends unaware of nested structs and collections; `src/regex.rs` holds the regex syntax and its translations.

### Not in 1.1

- Floats; lists of maps, maps of maps and deeper nesting; list-of-structs fields whose items have map fields; the `pkg` module.
- Tested on Bash 3.2, Bash 5 and PowerShell 7 on macOS. Windows PowerShell 5.1, Ubuntu and Alpine are covered by the CI workflow, which hasn't run yet.

## 1.0.0 — 2026-11-09

The first stable release. From here on, a script that compiles with Nalu 1.x keeps compiling with every later 1.x, and keeps doing the same thing.

### The language

- One typed `.nalu` file compiles to Bash for Linux (4.3+), Bash for macOS (the stock 3.2) and PowerShell for Windows (5.1+), with no runtime on the target.
- **Failures are loud by default.** Anything that can fail stops the script, or makes the enclosing function fail, unless you handle it with `catch`: `x catch default`, `x catch fail "message"`, `x catch ignore`, `x catch { … err … }`, `try { … } catch …`. The compiler infers which functions can fail; `nalu check --explain` lists every line that can.
- **Commands are safe by construction.** `$ cmd {x}` passes `{x}` as exactly one argument, `$(cmd)` captures output, `value | $ cmd` sends stdin, and every program a script uses is checked before it starts.
- **Scripts are command-line tools.** `param` with a `///` comment becomes a typed, validated flag with `--help`; `secret` parameters also come from `NALU_<NAME>` and never show up in arguments or logs; every script takes `--dry-run` and `--verbose`.
- Types like Go (`[]str`, `map[str]int`), structs with methods, enums with exhaustive `match`, `defer`, `retry`, `parallel for`, `test` blocks, and a standard library for files, paths, HTTP, JSON, hashes, archives, time, logs and prompts.

### Tooling

- `nalu run`, `build`, `check` (`--json`, `--explain`, `--stdin`), `fmt`, `doc`, `test`, `emit` and `ide`.
- `nalu fmt`: a formatter that keeps your line breaks and comments.
- `nalu doc`: the documentation of any module, function, type or keyword; `docs/stdlib.md` is generated from it.
- A VS Code extension: highlighting, live checks, ⚡ on every line that can fail, types on hover, completion by type, signature help, go to definition, format on save, and GitHub Copilot tools and instructions.
- Every compiler message and everything a generated script prints is in English and identical on the three targets.

### Changes from the 0.x previews

The previews had more than one way to do several things. 1.0 keeps one, and the compiler points to it when you use another:

| 0.x | 1.0 |
|---|---|
| `x or default`, `try { } or …` | `x catch default`, `try { } catch …` |
| `str[]`, `map<str, int>` | `[]str`, `map[str]int` |
| `m.get(k)`, `m.set(k, v)` | `m[k]`, `m[k] = v` |
| `s.is_empty()`, `xs.is_empty()` | `s == ""`, `xs.len() == 0` |
| `n.to_str()` | `"{n}"` |
| `p.exists`, `p.is_dir`, `p.is_file`, `p.size`, `p.modified` | `fs.exists(p)`, `fs.is_dir(p)`, `fs.is_file(p)`, `fs.size(p)`, `fs.modified(p)` |
| `path.join(a, b)` | `a / b` |
| `fail("message", code: 3)` | `fail "message", code: 3` |

`or`, `and` and `not` are reserved: the compiler explains that logic is `||`, `&&` and `!`. Path properties (`.name`, `.stem`, `.ext`, `.parent`) now only look at the text and never fail. The documentation moved to English (`README.pt-BR.md` stays as the Portuguese entry point), and the pre-1.0 design material moved to `docs/history/`.

### Fixed before 1.0

- Division by zero fails with `division by zero` on every target (Bash used to print a warning and go on, PowerShell stopped with its own message).
- `~` in commands: unquoted it is the home folder on every target, quoted it is text, and a `path` value starting with `~` is expanded when passed to a program.
- The Windows script is pure ASCII with no BOM, so `powershell -c "irm URL | iex"` works; a successful `irm | iex` no longer closes the console.
- With `--dry-run` on Windows, `fs.move` of a real folder into a temporary one is only shown (it used to happen, and `defer fs.rm(tmp)` could then delete the folder). Moving a folder to another drive works.
- `nalu build` shows the warnings of every target (a Windows-only `W010` used to be hidden); `nalu test file.nalu` works with a bare file name; `time.now_ms()` has millisecond precision on macOS.
- Clearer errors for `not x`, `} elif`, C-style `for (…)` and lowercase type names.

### Not in 1.0

- Floats, nested collections (lists of lists, structs inside structs, maps with non-scalar values).
- The `regex`, `encoding` and `pkg` modules: the compiler refuses them (E090) until they exist on every target.
- Tested on Bash 3.2, Bash 5 and PowerShell 7 on macOS. The CI workflow is set up to also run every case on Ubuntu, Alpine (BusyBox tools) and Windows PowerShell 5.1; until it has run there, treat those as untested.
