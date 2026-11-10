# Architecture

How the `nalu` compiler turns one `.nalu` file into Bash and PowerShell, and how the pieces around it (runtime helpers, formatter, editor support, tests) fit together. This page is for people who work on the compiler. For the language itself, see [language.md](language.md); for the reasons behind the choices, see [decisions.md](decisions.md).

## Principles

- **Rust 2021, zero dependencies.** Only `std`. One binary, `nalu`, for Linux, macOS and Windows.
- **Semantics are decided once.** The frontend lowers the program to a flat, typed IR ([`src/ir.rs`](../src/ir.rs)) for one target. Every semantic decision (temporaries, evaluation order, how a `catch` runs, `match` as a value, `defer`, `parallel`, named arguments, `os`) is made there. The backends only spell the IR in their shell's syntax ([D08](decisions.md#d08--flat-ir-a-normal-form-with-explicit-fallibility)).
- **The runtime is real shell code.** Helpers live in [`runtime/bash/`](../runtime/bash) and [`runtime/ps/`](../runtime/ps), are embedded with `include_str!`, and a generated script carries only the helpers it uses (D09).
- **User input never crashes the compiler.** The lexer and parser recover and collect diagnostics; the frontend runs on a thread with a 256 MB stack, and the parser caps nesting at 64 levels. A panic that slips through becomes `E999` (internal compiler error).
- **Stable diagnostic codes** (`E012`, `W001`), rendered like rustc or as JSON ([`src/diag.rs`](../src/diag.rs)). Spans are byte offsets; columns are counted in characters.
- **Tools reuse the compiler.** The formatter, editor queries, `check --explain`, `nalu doc` and the Copilot integration are built on the same lexer, checker and stdlib tables. Nothing reimplements the language (D28, D29).

## Pipeline

```text
x.nalu
  │
  ▼
lexer.rs ────► tokens ─────────────────────────────► fmt.rs ──► nalu fmt
  │
  ▼
parser.rs ───► ast::File + diagnostics
  │
  ▼
check.rs ────► check::TProgram + diagnostics ──────► lower::explain ──► nalu check --explain
  │            check::Index ───────────────────────► ide.rs ──► nalu ide, nalu doc, VS Code
  ▼
lower.rs ────► ir::Program (one per target)
  │
  ▼
nest.rs ─────► ir::Program with only flat structs and collections
  │
  ├─► bash/ + runtime/bash/ ──► x.linux.sh   (Bash 4.3+)
  │                             x.macos.sh   (Bash 3.2)
  │
  └─► ps/ + runtime/ps/ ──────► x.windows.ps1 (Windows PowerShell 5.1+) + x.windows.cmd
```

| Stage | Files | Input → output |
|---|---|---|
| Lexer | `src/lexer.rs` | source → tokens |
| Parser | `src/parser.rs`, `src/ast.rs` | tokens → untyped AST + diagnostics |
| Checker | `src/check.rs` | AST → typed, resolved tree (`TProgram`) + diagnostics; optionally the editor index |
| Lowering | `src/lower.rs` | typed tree + target → `ir::Program` |
| IR passes | `src/nest.rs` | IR → IR: structs inside structs flattened, nested collections encoded ([below](#ir-passes)) |
| Bash backend | `src/bash/`, `runtime/bash/` | IR → script for `linux` or `macos` |
| PowerShell backend | `src/ps/`, `runtime/ps/` | IR → `.ps1` and a `.cmd` launcher |
| CLI | `src/main.rs` | arguments → [commands](#cli) |

The library surface between these stages is small:

```rust
// src/lib.rs: the whole frontend for one target
pub fn compile(source: &str, file: &str, embed_root: &Path, target: Target) -> Compiled;
pub struct Compiled { pub program: Option<ir::Program>, pub diags: Vec<diag::Diag> }

// src/bash/mod.rs and src/ps/mod.rs
pub fn generate(program: &ir::Program, mode: Mode) -> String;   // Mode::Script or Mode::Tests

// src/ps/mod.rs
pub fn launcher(script_name: &str) -> String;                    // x.windows.cmd
```

`program` is `None` when any diagnostic is an error. The CLI first joins the files of a script into one text with an offset map (`Source` in `main.rs`): `x_test.nalu` with `x.nalu` for tests, and every file brought in by `use "…"` (D37), found recursively and loaded once; the `use` lines of the appended files are blanked out with spaces, so positions don't move. Diagnostics, `--json` and the editor queries map each position back to its file. `embed_root` is the folder `embed("…")` reads from. `Mode::Tests` generates the declarations and `test` blocks only, for `nalu test`. The PowerShell backend returns text with `\n`; the CLI writes the `.ps1` with CRLF line endings, as plain ASCII (a BOM only when a `native` block has non-ASCII text).

## Module map

Sizes are approximate line counts, excluding inline `#[cfg(test)]` modules.

| File | Responsibility | Lines |
|---|---|---|
| `src/lib.rs` | Module list and `compile()` | 50 |
| `src/diag.rs` | `Span`, `Diag`, codes, rustc-style and JSON rendering | 150 |
| `src/target.rs` | `Target` (`linux`, `macos`, `windows`) and `Mode` | 70 |
| `src/stdlib.rs` | The stdlib tables: functions, methods, path properties, builtins, modules, keywords, removed names. The single source of truth for the API, its docs, fallibility and `--dry-run` behavior | 360 |
| `src/lexer.rs` | Tokens, significant newlines, strings with `{…}` parts, `$` command lines, durations | 1,350 |
| `src/ast.rs` | Untyped syntax tree | 330 |
| `src/parser.rs` | Resilient recursive-descent parser | 1,690 |
| `src/check.rs` | Name resolution, types, fallibility inference, `match` coverage, nested assignment, diagnostics, editor index | 4,240 |
| `src/regex.rs` | Nalu's regex syntax: parser, the single-way check, translation to POSIX ERE and .NET (D32) | 910 |
| `src/lower.rs` | Typed tree → IR for one target; `match` with guards as `if` chains; `explain` for `check --explain` | 1,820 |
| `src/ir.rs` | The IR: the contract between frontend and backends | 520 |
| `src/nest.rs` | IR passes: structs inside structs flattened (D35), nested collections encoded (D34) | 920 |
| `src/bash/` | Bash backend: `gen` (frames, statements, script assembly), `call` (calls, methods, stdlib, `parallel`), `store` (copy-by-value into destinations), `expr`, `cmd` (`$`, `$( )`, `--verbose`, `--dry-run`), `params` (command-line parser), `quote`, `val`, `runtime` | 4,900 |
| `src/ps/` | PowerShell backend: `gen` (with the `parallel` runspace pool), `runtime`, `syntax` (a minimal 5.1 syntax checker for machines without PowerShell) | 2,470 |
| `runtime/bash/*.sh` | Bash helpers: `core`, `str`, `list`, `map`, `fs`, `sys`, `data` (7 files, 149 helpers) | 2,090 |
| `runtime/ps/*.ps1` | PowerShell helpers: `core`, `text`, `list`, `path`, `fs`, `sys`, `data`, `cmd`, `params` (9 files, 145 helpers) | 2,060 |
| `src/fmt.rs` | `nalu fmt` | 660 |
| `src/ide.rs` | Hover, completion, signature help, go-to-definition, AI context, `nalu doc`, the generated stdlib reference | 1,500 |
| `src/main.rs` | The CLI | 1,050 |
| `tests/e2e.rs` | End-to-end harness over `tests/cases/`, `tests/errors/`, `examples/`, `tests/pacote/` and the CLI | 910 |
| `src/bash/tests.rs`, `src/ps/tests.rs` | Backend tests from hand-built IR, stdlib coverage, runtime test runners | 1,420 + 900 |
| `editors/vscode/` | VS Code extension: `extension.js`, `copilot.js`, `check-json.js` | 800 |
| `.github/workflows/ci.yml` | CI matrix | 65 |

## Frontend

### Lexer

- Newlines are significant: one statement per line. A newline is dropped after an operator, `,`, `(`, `[`, `{` or `catch`, and before a line that starts with `.` (method chains). Consecutive newlines collapse into one.
- In `"…"` strings, each `{expr}` part is kept as raw text with its source offset; the parser re-parses it, so an error inside `{ }` points at the right column. An empty `{}` is literal (templates for `fmt`). `'raw'` strings have no escapes and no interpolation. `"""` drops the first newline, the closing line and the common indentation.
- `$ …` switches to command mode until the end of the line, an unquoted ` catch `, a `{` that ends the line (the block of `if $ cmd {`), or `//`. `$(` captures up to its matching `)`.
- `///` doc comments attach to the next token. Durations such as `1m30s` are single tokens, stored in milliseconds.
- Errors become `E005`/`E006`, and lexing continues.

### Parser

Recursive descent over the grammar in [language.md](language.md). On an error it reports `E005` (or `E006` with the Nalu form of what you probably meant, such as `||` for `or`), skips to the end of the line or the `}` that closes the block, and continues. Every loop consumes at least one token, and nesting depth is capped, so no input makes it hang or overflow. The result is always `(ast::File, Vec<Diag>)`.

### Checker

`check::check` turns the AST into `TProgram`, a tree that is typed and resolved:

- Every variable gets a unique `VarId`; calls resolve to a user function, a stdlib entry, a builtin or a method. Named arguments and defaults are filled in, so every `TCall.args` is positional and complete.
- It checks types and units (durations), `let` versus `var`, `match` exhaustiveness and unreachable arms, `secret` flow, what a `parallel` body may touch, and `err`/`self`/`break` out of place.
- `regex` patterns must be fixed text; `src/regex.rs` parses them, and for the functions that return matched text checks that the pattern has a single way to match (D32). The backends translate the same parsed pattern. After a reported error, unknown types use an internal marker that is compatible with everything, which prevents cascades.
- Frequent messages live in one module (`check::msg`).

**Fallibility** is inferred, not declared ([D02](decisions.md#d02--fail-fast-by-default-or-to-recover)). Stdlib entries and methods carry a static `fallible` flag; for user functions, `infer_fallible` computes a fixpoint. It starts with every function infallible, then repeatedly scans the body of each function still marked infallible for an unhandled failure (`reasons_stmts`), given the current table, and marks it fallible. Functions only flip from infallible to fallible, so the loop ends after at most n + 1 passes for n functions, and a recursion cycle with no real source of failure stays infallible. The same walk records each function's reasons, which the editor shows as "It fails when …". Checks that need the final answer run afterwards: `E040` (`catch` on something that cannot fail) and `E060` (`assert_fails` on something that never fails).

**The editor index.** `check::check_indexed` also returns a `check::Index`, built while checking:

- `syms: Vec<(Span, Sym)>`: what each name in the source is. `Sym` is a variable, user function, stdlib function, builtin, method, struct field, property, struct, enum, enum case, module or type name.
- `exprs: Vec<(Span, Ty)>`: the type of every expression.
- Each `VarInfo` records `vis`, the byte range where the name is visible, which is what scope-aware completion uses.

The checker also runs on an AST with syntax errors (the parser recovers), so the editor gets types while the code is still incomplete.

### Lowering

`lower::lower` turns `TProgram` into `ir::Program` for one target and guarantees the IR's invariants:

- **A-normal form.** Every call, capture, command and collection literal becomes an `Rvalue` in a `Let`, `Assign` or `Call`; temporaries are `__t1`, `__t2`, and so on.
- **`OnFail` on everything that can fail**, `Propagate` when there is no `catch`. A `catch` whose left side contains nested fallible parts (`f(g()) catch x`) becomes a slot variable plus a `Try`, so the handler covers all of them.
- **Unique names** in each function, test and the top level (`x`, `x_2`); functions never reuse a global's name.
- **Target specifics resolved.** `os` is a constant: the dead branch of `if os == …` disappears and `match os` is reduced. `W010` (a program that doesn't exist on this target, like `grep` on Windows) is reported here.
- **Sugar removed.** `embed` becomes a literal, `+=` becomes a binary operation, `if`/`match`/`parallel` used as values become a `Let` plus assignments, a `while` with an impure condition becomes `while true` + `if !c { break }`, and `&&`/`||` with an impure right side become nested `if`s (short-circuiting is preserved).
- **Index checks.** `xs[i]` and `xs[i] = v` get a bounds check that fails with `index … is out of range` (D13).
- **`match` in order.** A `match` with guards, bindings or struct patterns becomes a chain of `if`s that test each arm in turn (with a flag when a guard can send control to the next arm); a plain `match` stays a `Match` statement, which the backends turn into `case`/`if` (D33).
- **Ready-made data.** The `--help` text (`usage`), identical on every target, and `deps`, the fixed program names used in commands, which the script checks for at startup (exit code 127 when one is missing).

The checker already rewrote nested assignment: `grid[i][j] = v` and `groups[k].push(x)` arrive as a copy of the inner value into a hidden variable, the change, and a write back, with the index computed once.

### IR passes

`nest.rs` runs on the IR of every target, after lowering, so the backends only ever see structs of scalars and collections and collections of scalars (or of structs):

- **Structs inside structs** (D35) are flattened: `svc: Service` becomes the fields `svc__name` and `svc__port`. `d.svc.port` reads `d.svc__port`; `d.svc` used as a whole value is rebuilt from its fields; `d.svc = s` and `Deploy { svc: s }` write each field. Lists of such structs need nothing else. A **list of structs as a field** (D38) becomes one list per field of the item (`services__name`, `services__port`): `cfg.services[i].name` reads `services__name[i]`, a whole item or list is rebuilt with a loop, a written list is split with another, and `push`/`pop` on the field change a rebuilt copy that is written back after the statement.
- **Nested collections** (D34): an inner list is stored as one text, each item followed by `\x1e`, so a `[][]int` is a `[]str` and a `map[str][]str` a `map[str]str`. Whole-collection operations only change type; taking an item out (`xs[i]`, a loop variable, `pop`/`first`/`last`/`find`, `m[k]`, a lambda's parameter) inserts the builtin `dec_list`, and putting one in (`xs[i] = v`, `push`, `contains`, literals, a lambda returning a list) inserts `enc_list`. Equal lists have equal texts, so `contains`, `index_of` and `unique` keep working. A `catch` default stays lazy: what it needs is computed inside the handler.

Both passes skip a program that doesn't need them.

`lower::explain` serves `nalu check --explain`: it runs the same `reasons_stmts` walk over the top level, every function and every test, and reports each point that can fail without `catch` and where the failure goes, as in ``app.nalu:10:16: `fs.read` can fail → `version` fails``.

## The IR

[`src/ir.rs`](../src/ir.rs) documents seven rules at the top. The essentials:

- **`Expr` is pure, infallible and scalar.** Literals, variables, fields, list index, arithmetic, comparison, logic, interpolation, `len`. A collection appears in an `Expr` only as a `Var` or `Field`. Every `Expr` carries its `Ty`.
- **`Rvalue`** is what may appear on the right of a `Let` or `Assign`: an `Expr`, a list, map or struct literal, a `Call`, a `Capture` (`$(…)`) or a `CommandOk` (`$ cmd` used as a condition).
- **`OnFail`** says what happens when an operation fails: `Propagate`, `Ignore`, `Default(Rvalue)` (which may itself be fallible, giving chains), `Fail { message, code }`, or `Block { err, body, value }`. `Propagate` means: in a function, the function fails; inside `try`, jump to its handler; inside `retry`, try again; inside `parallel`, collect the error; at the top level, print `error: message` to stderr and exit with the error's code.
- **`StmtKind`**: `Let`, `Assign`, `Call`, `Command`, `If`, `While`, `ForRange`, `ForList`, `ForMap`, `Match`, `Try`, `Retry`, `Parallel`, `Fail`, `Return`, `Break`, `Continue`, `Exit`, `Defer`, `Native`, `Assert`, `AssertFails`.
- **Defined arithmetic and comparison.** `Div` truncates toward zero, `Mod` takes the dividend's sign, and text compares ordinally, by code point ([D11](decisions.md#d11--ordinal-text-comparison-on-both-targets)), whatever the shell would do by default.
- Every call's arguments are positional and complete; a method is a function `Type.method` with `self` as its first argument; assigning a collection copies it.

`nalu emit x.nalu --ir` prints the IR of a file (as Rust `Debug` output).

### An example through the pipeline

```nalu
let user = env.get("USER") catch env.get("USERNAME") catch "x"
print("hi, {user}")
```

The IR, abridged from `nalu emit --ir`:

```text
Let user: Str = Call Std(env.get)("USER")
                  on_fail Default(Call Std(env.get)("USERNAME")
                                    on_fail Default(Expr "x"))
Call Builtin(print)(Interp["hi, ", Var user]) on_fail Propagate
```

Bash (`macos` and `linux` agree here):

```bash
# line 1
if __env_get USER; then
  user=$__ret
elif __env_get USERNAME; then
  user=$__ret
else
  user=x
fi
# line 2
printf '%s\n' "hi, ${user}"
```

PowerShell:

```powershell
# line 1
try {
    $o_user = (__EnvGet ('USER'))
} catch {
    try {
        $o_user = (__EnvGet ('USERNAME'))
    } catch {
        $o_user = 'x'
    }
}
# line 2
[Console]::Out.Write([string]('' + 'hi, ' + $o_user) + "`n")
```

### Why the backends agree

Bash and PowerShell differ on almost everything that matters to a script: error handling, scoping, how a function returns a list, string comparison, integer division, how arguments reach a program. If each backend walked the AST, each would make those decisions separately, and they would drift; Stela's QA showed exactly that (D08). Nalu removes the room for disagreement:

1. **No decision is left to the backends.** The order of evaluation is spelled out by temporaries, every failure point names its handler, names are already unique (no scope resolution), `os` is already resolved, defaults are filled in, and every node is typed. A backend decides *how* to write each instruction, never *what* it means.
2. **The instruction set is small and closed.** Each backend is a translation of about twenty statement kinds, five failure modes and a pure expression language, and each can be tested from hand-built IR (`src/bash/tests.rs`, `src/ps/tests.rs`) without the frontend.
3. **Where the shells differ, the IR says which behavior wins**: ordinal comparison, truncating division, `exit(n % 256)`, UTF-8 without a BOM and `\n` for every file write, and one error text for both runtimes (D27).
4. **What construction can't guarantee, tests check.** Every `v1` stdlib entry must be implemented by both backends (a coverage test fails on any `__unimplemented`), and every end-to-end case runs on Bash 3.2, Bash 4.3+ and PowerShell (except cases marked Unix-only), with output compared byte for byte against the same expected file. A target-specific `x.windows.out` exists only where the difference is intended (`match os`, for example).

## Runtime helpers

The code a script needs at run time (reading files, JSON, HTTP, maps on Bash 3.2, the command-line parser) lives in ordinary shell files, one block per helper:

```bash
# @fn __fs_size
# @needs __tilde __err __stat_size
# @target all
__fs_size() {
  …
}
```

- `# @fn NAME` starts a block; `# @needs A B` lists the helpers it calls. `# @target linux|macos|all` exists only in the Bash files: the same helper may have one body per target, as `__stat_size` (GNU `stat -c` versus BSD `stat -f`) or `__par_slot` (`wait -n` versus waiting on the oldest job) do.
- Every Bash helper name starts with `__`; PowerShell helpers are `__PascalCase` (`__FsRead`). No helper runs `eval` on user data. PowerShell helpers use only 5.1 syntax.
- `src/bash/runtime.rs` and `src/ps/runtime.rs` embed the files with `include_str!` and pick what a script needs. The Bash generator records each helper it calls (`Gen::need`), and `runtime::emit` adds the transitive closure of `@needs`; an unknown name panics, which the tests catch. The PowerShell side scans the generated code for whole-word mentions of helper names, then closes over `@needs`. Both emit the selected helpers in file order, so output is deterministic.
- The result stays small: the Bash tests keep `examples/hello.nalu` under 100 generated lines per target and fail if a script includes a helper it never calls.

The helpers have their own unit tests, run by `cargo test` and runnable alone:

```bash
bash runtime/bash/tests.sh macos                     # any Bash, 3.2 or newer
/opt/homebrew/bin/bash runtime/bash/tests.sh linux   # a Bash 4.3+
pwsh -NoProfile -File runtime/ps/tests.ps1
```

`tests.sh` loads the helpers for one target with the same `@target` filter as the generator, runs one test per helper and fails if any helper has no test.

## Targets

| Target | Output | Shell |
|---|---|---|
| `linux` | `x.linux.sh` | Bash 4.3+ (`#!/usr/bin/env bash`), including BusyBox userlands such as Alpine |
| `macos` | `x.macos.sh` | Bash 3.2, the `/bin/bash` macOS ships |
| `windows` | `x.windows.ps1` + `x.windows.cmd` | Windows PowerShell 5.1+, also PowerShell 7 |

`Target::parse` also accepts `darwin`, `mac` and `win`. `nalu run` compiles for the machine it runs on.

### Bash: the shared model

- **Prologue.** `set -o pipefail` and a fixed `IFS`, **no `set -e`**: every failure is checked where it happens. The script makes sure the locale is UTF-8 by testing the effect (`${#p} == 1` for `p='ç'`), trying `C.UTF-8` and `en_US.UTF-8` if needed. Sections are marked `# ---- runtime ----`, `# ---- parameters ----`, `# ---- functions ----` and `# ---- script ----`, and each statement is preceded by `# line N`.
- **Functions** are `fn_name`. Scalars arrive as `local a=$1`; results come back in `__ret` (scalar), `__ret_a` (list), `__ret__field` (struct) or `__ret_m__k`/`__ret_m__v` (map). Failure is `return 1` with `__err_msg` and `__err_code` set; at the top level, `|| __fail` prints `error: …` and exits with the code.
- **`try` and `retry`** are `while :` loops: a failure inside is `break` (to the handler) or `continue` (next attempt), so the user's `return`, `break` and `continue` keep working (D18). For example, `print(xs[i] catch "none")` becomes:

  ```bash
  __t1=''
  __v1=1
  while :; do
    if (( i < 0 || i >= ${#xs[@]} )); then
      __err_msg="index ${i} is out of range (length ${#xs[@]})"; __err_code=1
      break
    fi
    __t1=${xs[i < 0 ? 1<<62 : i]}
    __v1=0; break
  done
  if (( __v1 )); then
    __t1=none
  fi
  ```

- **Data.** Structs are flattened into one variable per field (`s__field`); a list of structs is one array per field. A map `m` is two dense arrays in insertion order, `m__k` and `m__v`, plus an index from key to position (see the table below).
- **`defer`** pushes onto a global stack, `__defers`, with a mark per function frame; `trap '__run_defers 0' EXIT` runs what is left on exit, and `INT`, `TERM` and `PIPE` exit with 130, 143 and 141.
- **`parallel`** runs each task in a background subshell; results and errors go to files in a temporary folder and are read back in task order.
- **Quoting.** Literals are single-quoted (`'\''` for a quote); `{x}` in a command is always exactly one argument and `{list}` alone is one argument per item. No `eval` touches user data.
- **Dependencies.** `__need git docker` at startup checks the fixed program names from `deps` and exits with 127 listing the missing ones (skipped under `--dry-run`).

### Bash: linux versus macos

The two Bash targets share the generator; where Bash 4.3 offers something better, the `linux` script uses it ([D07](decisions.md#d07--linux-and-macos-get-different-bash)).

| | `linux` (Bash 4.3+) | `macos` (Bash 3.2) |
|---|---|---|
| Map index | `declare -A m__i` | one variable per key, `__m<id>_<encoded key>`, read with `${!name}`, written with `printf -v` |
| List parameters | a `local -n` reference to the caller's array ([D25](decisions.md#d25--lists-by-reference-on-the-linux-target-2026-11-09)) | count plus items, copied into a local array, then `set --` to empty `$@` ([D16](decisions.md#d16--never-leave-a-large-list-in--during-a-loop-bash-32)) |
| `upper`, `lower` | `${x^^}`, `${x,,}` | a helper that maps characters without forking `tr` |
| `parallel` with `max` | `wait -n` | wait for the oldest job |
| `fs.size`, `fs.modified` | `stat -c` (GNU, BusyBox), falling back to `date -r` | `stat -f` (BSD) |
| `time.now`, `time.now_ms` | `printf '%(%s)T'`, `$EPOCHREALTIME` | `date +%s`; `now_ms` asks the `perl` that ships with macOS for milliseconds (whole seconds without it) |

The list-parameter difference, for `fn total(xs: []int) -> int` called as `total(nums)`:

```bash
# linux: the caller passes the array's name
fn_total() {
  local -n __r1_xs=$1
  set --
  …
}
fn_total nums

# macos: the caller passes the count and the items
fn_total() {
  local -a xs=("${@:2:$1}")
  set --
  …
}
fn_total "${#nums[@]}" "${nums[@]}"
```

### PowerShell

- **Prologue.** `Set-StrictMode -Version 2`, `$ErrorActionPreference = 'Stop'`, UTF-8 without BOM for console output, and the whole script body inside `try { … } catch { … } finally { … }`: the `catch` converts any error with `__ErrOf`, prints `error: …` and sets the exit code; the `finally` runs the `defer` stack.
- **Names.** Variables are `$o_name`, functions `fn_name`. PowerShell names are case-insensitive, so an uppercase letter is encoded (`A` → `_a_`). Top-level variables used inside functions are `$script:o_name`.
- **Functions** return through `$script:__ret` and are always called as statements, never inside `$( )` or on the right of an assignment: that would unroll a one-item list and capture the output of external programs.
- **Failures are exceptions** thrown by `__Throw`, with the code in `Data['nalu_code']`. `Propagate` generates nothing: the exception rises to the right `catch` (a `try`, `retry`, `parallel`, the calling function or the top level). Every other `OnFail` becomes a `try { } catch { }` around the operation.
- **Data.** Lists are `List[object]`; maps and structs are `OrderedDictionary` with an ordinal key comparer (`@{}` would ignore case). `match` becomes `if`/`elseif`, since a `break` inside `switch` would stop the `switch` instead of the loop.
- **External programs** always run through `System.Diagnostics.Process`, never `& prog` ([D17](decisions.md#d17--powershell-runs-external-programs-through-systemdiagnosticsprocess)). The command line is built with MSVCRT quoting rules (`__QArg`); redirections and pipes copy bytes; `$ echo …` becomes a one-line write, as in Bash; `.cmd` and `.bat` go through `cmd.exe /d /s /c`.
- **Text and numbers.** Integers are `[long]`; division truncates through `[decimal]`, never through `double`. Comparison is ordinal by code point, including characters outside the BMP (`__Cmp`). Files are read and written as UTF-8 without a BOM and with `\n`.
- **`parallel`** runs its tasks at the same time in a `RunspacePool` (Windows PowerShell 5.1 has it; `Start-Job` was ruled out because it starts another process without the script's functions). Each task is a function; every `__*` and `fn_*` function is copied into an `InitialSessionState`, the script's variables into each task as deep copies (like the Bash subshell), and the enclosing function's variables are read by reference (the checker forbids changing them). The pool has no host, so an `exit` in a task ends only that task. Errors are joined in task order with the first failed task's code, as in Bash. Tasks that call `sys.cd` or `env.set` (found through the call graph) run one at a time, because the folder and the environment belong to the process; after a `parallel`, the parent's folder and environment are restored. Without a pool (Constrained Language Mode), the tasks run one after another in the same runspace.
- **The file.** The `.ps1` is plain ASCII with CRLF and no BOM: every character outside ASCII in a literal becomes `$([char]0x…)` (`[char]::ConvertFromUtf32` above the BMP), so typographic quotes (`‘ ’ “ ”`, which PowerShell treats as real quotes) can't break a string, Windows PowerShell 5.1 reads the file correctly without a BOM, and `irm … | iex` works (a BOM breaks it). Only a `native` block with non-ASCII text makes the CLI add a BOM. The final `exit` runs only when the script is a file (`$PSCommandPath`), so `irm | iex` in a console does not close the window on success. `x.windows.cmd` runs it with `powershell -NoProfile -ExecutionPolicy Bypass -File`.

## Tooling

### CLI

| Command | What it does |
|---|---|
| `nalu run x.nalu [-- args…]` | Compiles for this machine, writes the script to a temporary folder, runs it with `/bin/bash` or PowerShell (`NALU_PWSH`, `pwsh`, `powershell`), and passes stdio and the exit code through |
| `nalu build x.nalu [--target a,b] [-o dir]` | Writes `x.linux.sh`, `x.macos.sh`, `x.windows.ps1` and `x.windows.cmd` (all targets into `dist/` by default); `.sh` files are made executable |
| `nalu check x.nalu [--json] [--explain] [--stdin]` | Compiles for all three targets and merges the diagnostics, so target-specific warnings appear. `x_test.nalu` is checked together with `x.nalu`. `--explain` lists every point that can fail without `catch` (JSON: `{diagnostics, fallible, fallible_fns}`); `--stdin` reads the source from standard input |
| `nalu emit x.nalu [--target T] [--ir]` | Prints the generated script, or the IR |
| `nalu test [folder\|file] [--target T]` | Runs test packages: `x_test.nalu` concatenated with `x.nalu` (whichever declares `param` goes first; diagnostics are mapped back to the right file), or a file with its own `test` blocks. Generates with `Mode::Tests` |
| `nalu fmt <file\|folder>… [--check]`, `nalu fmt --stdin name.nalu` | Formats in place, checks, or filters standard input |
| `nalu doc [name…] [--all]` | Documentation for a module, function, type, member or keyword; `--all` prints the full stdlib reference |
| `nalu ide hover\|complete\|signature\|definition\|context x.nalu --at L:C [--stdin]` | Editor queries, answered as JSON |

Exit codes: 0 success; 1 compile errors, failed tests or an unreadable file; 2 wrong usage. `run` returns the script's own code, and `test` the worst code among its packages.

### Formatter

`src/fmt.rs` works on the lexer's tokens and never on the AST, like `gofmt`: it keeps every line break and fixes the rest. Input the parser rejects is returned unchanged, with the diagnostics. Each token comes out with its exact source text, so numbers, strings and `$` commands (where whitespace is meaningful) are untouched. The rules: 4 spaces per indentation level (brackets left open at the end of a line add one level, however many there are), one extra level for a continuation line (after a trailing operator, or starting with `.`), one space around binary operators and after `,` and `:`, Go-style types (`[]str`, `map[str]int`), at most one blank line in a row, and aligned columns in exactly two places (trailing `//` comments on consecutive lines, and `=>` in consecutive one-line `match` arms). CRLF line endings and a BOM are preserved. Its tests check idempotence and that the token sequence never changes, over every `.nalu` file in the repository and every code block in `docs/`.

### Editor queries and `nalu doc`

`src/ide.rs` answers from the checker's `Index`, never from a parser of its own:

- **Hover** finds the `Sym` under the cursor and describes it: the declaration with its type, the `///` doc, and for a function that can fail, why. Keywords such as `catch`, `defer` and `retry` come from `stdlib::KEYWORDS`.
- **Completion** after `x.` blanks out the partial name (offsets stay valid), re-checks, and lists the members of the type of the expression that ends before the dot. Elsewhere it lists the variables whose `vis` range contains the cursor, plus functions, types, modules and keywords. At the start of a `match` arm it offers the cases not yet covered, and `_`.
- **Signature help** and **go-to-definition** use the same lookups.
- **AI context** (`nalu ide context`) returns the language rules that models most often get wrong, as traits, and as snippets the names visible at the cursor with their types and the stdlib modules and methods relevant there; a short file also gets an idiomatic example script.
- **`nalu doc`** renders pages from the stdlib tables: ⚡ marks what can fail, ✎ what writes (and is skipped under `--dry-run`). `nalu doc --all` produces [stdlib.md](stdlib.md); a test fails when that file is stale.

Each query is one process that analyzes the whole file, on the same large-stack thread as `compile`. A 3,500-line file takes about 10 ms, which is why there is no LSP server ([D28](decisions.md#d28--formatter-and-smart-editor-both-in-the-compiler-2026-11-09)).

### VS Code extension

[`editors/vscode/`](../editors/vscode) has no dependencies and no build step; every answer comes from the `nalu` binary (from the `nalu.path` setting, else `target/release` or `target/debug` in the open folder, else `PATH`).

- `extension.js` lints with `nalu check --stdin --json --explain` while you type (300 ms after you stop), on open and on save; marks lines that can fail with ⚡; registers hover, completion, signature help, definition and formatting providers that call `nalu ide` and `nalu fmt --stdin`; and adds commands to check, run and build.
- `check-json.js` parses the compiler's JSON (both the plain list and the `--explain` object) and converts columns; it is plain Node, testable without VS Code.
- `copilot.js` integrates GitHub Copilot ([D29](decisions.md#d29--copilot-learns-nalu-from-the-compiler-2026-11-09)): agent-mode tools `nalu_check`, `nalu_docs` and `nalu_format`; a context provider for inline suggestions fed by `nalu ide context`; and chat instructions (`copilot/nalu.instructions.md`, mirrored in `.github/instructions/`, with a test that keeps the copies identical and their example compiling and formatted).

## Testing

```bash
# Everything. On a Mac with Homebrew's bash and powershell, every case runs on
# Bash 3.2 (macos target), Bash 5 (linux target) and PowerShell 7:
NALU_PWSH=/opt/homebrew/bin/pwsh cargo test --release

# Only the end-to-end suite
cargo test --release --test e2e
```

On macOS, `brew install bash powershell` provides the extra shells. The harness finds a Bash 4.3+ through `NALU_BASH4` or at `/opt/homebrew/bin/bash` and `/usr/local/bin/bash`, and PowerShell through `NALU_PWSH`, `pwsh` or `powershell`. Without them it still generates and validates every script (`bash -n`; the minimal 5.1 checker in `src/ps/syntax.rs`) and prints a warning instead of running it.

| Layer | Where | What it checks |
|---|---|---|
| Unit | `#[cfg(test)]` in the lexer, parser, checker, lowering, formatter, editor queries, CLI | Each stage in isolation |
| Backends | `src/bash/tests.rs`, `src/ps/tests.rs` | Hand-built IR → script → real shell, compared output; every `v1` stdlib entry implemented; the runtime unit tests |
| Cases | `tests/cases/` (44 programs) | Each program compiled for all targets and run on every available shell; stdout, exit code and stderr compared |
| Compile errors | `tests/errors/` (86 files) | The first line, `// error: E0xx` or `// warning: W0xx`, names the diagnostic `nalu check` must report |
| Examples | `examples/` | Compile for all three targets, pass `bash -n`, and `nalu test examples` passes (on PowerShell too, when present) |
| Test packages | `tests/pacote/` | `nalu test` with failures on purpose: exit code 1 and output equal to `saida.txt`, on Bash and PowerShell |
| CLI and docs | `tests/e2e.rs` | Help, usage errors, `build` output files, `run` arguments and exit codes, `check --json`; [stdlib.md](stdlib.md) matches `nalu doc --all`; the Copilot instructions match |

A case is `x.nalu` plus `x.out` (expected stdout) and, optionally, `x.code` (exit code), `x.args` (one argument per line), `x.env` (`NAME=value` lines), `x.err` (lines that must appear in stderr, or with a leading `!`, must not appear anywhere), `x.linux.out`/`x.macos.out`/`x.windows.out` (a different expected output on one target) and `x.unix` (a marker: the case uses Unix programs and is skipped on PowerShell). The `.ps1` must also pass `tests/ps_syntax.ps1`, which uses PowerShell's own parser, rejects syntax that exists only in PowerShell 7, and requires the BOM. Each test collects all failures and reports them at once, with diffs.

**CI** ([`.github/workflows/ci.yml`](../.github/workflows/ci.yml)) runs the suite where scripts really run: Ubuntu (Bash 5 and PowerShell 7), macOS (Bash 3.2 and PowerShell 7), Windows (Windows PowerShell 5.1) and Alpine (Bash on BusyBox, to catch differences in `date`, `stat`, `find` and `sort`). On Linux it also runs `nalu fmt --check examples tests/pacote` and the runtime tests for the `linux` target, and outside Windows it times the benchmarks in `tests/bench/` (`laco`, `mapa`, `lista`).

## Making changes

- **A stdlib function.** Add the entry to `STDLIB` (or `METHODS`) in `src/stdlib.rs`: its signature in Nalu type syntax, `fallible`, `writes`, `v1` and a one-line doc. The checker, hover, completion and `nalu doc` pick it up from there. Implement it in both backends: a case in `std_call` (`src/bash/call.rs`) and one in `src/ps/gen.rs`, each usually calling a new runtime helper with its own unit test (`runtime/bash/tests.sh`, `runtime/ps/tests.ps1`). A function that writes must honor `--dry-run` (`__dry_skip` in Bash, `__DryW` in PowerShell). Add an end-to-end case and regenerate the reference with `nalu doc --all > docs/stdlib.md`. The coverage tests fail until both backends implement the entry.
- **A diagnostic.** Emit it from the checker with a new stable code and, when there is one, a help line that shows the fix as Nalu code; add a `tests/errors/` file whose first line names the code, and list the code in [language.md](language.md).
- **The IR.** It is the contract between the frontend and both backends: a new variant means lowering it, generating it in both backends, and testing it from hand-built IR in `src/bash/tests.rs` and `src/ps/tests.rs`. Keep the rules at the top of `src/ir.rs` true.
- **A behavior that differs between shells.** Decide the behavior in the IR or in the stdlib table, not in one backend, and add a case to `tests/cases/` so the differential run holds it in place.

The specifications the compiler was first built from, written in Portuguese, are in [history/specs/](history/specs/README.md).
