# Case corpus (`tests/cases`)

Each case is a small, focused program whose expected output was written **by hand from the reference**
([docs/language.md](../../docs/language.md) and [docs/stdlib.md](../../docs/stdlib.md)), before the
implementation existed. It is the acceptance test for the frontend (SPEC-001) and for the two backends
(SPEC-002/003): if a case fails, either the implementation is wrong or the reference is ambiguous (see
"Open questions" below).

The harness is `tests/e2e.rs` (`cargo test --test e2e`). Compile errors and warnings live in
`tests/errors/` and are described at the end.

## File convention

A case is a group of files that share a base name. The base name is the case name.

| File | Required | Content |
|---|---|---|
| `x.nalu` | yes | the program |
| `x.out` | yes (or the per-target variants) | expected stdout, byte for byte (`\n`; PowerShell's `\r\n` is normalized) |
| `x.linux.out`, `x.macos.out`, `x.windows.out` | no | expected stdout on that target, instead of `x.out` |
| `x.code` | no | expected exit code (default `0`) |
| `x.args` | no | script arguments, **one per line** (an empty line is an empty argument) |
| `x.err` | no | each non-empty line must appear in stderr; with a leading `!`, it must appear in **neither** stdout nor stderr |
| `x.env` | no | environment variables `NAME=value`, one per line |
| `x.unix` | no | empty marker: the case uses Unix programs (`printf`, `sh`, `cat`…) and is not run on PowerShell |

The program name (`x`) shows up in generated output, for example in the `usage:` line of `--help`
(`params_help.out`), so renaming a case can change its expected output.

## What the harness does

`tests/e2e.rs` loads every `tests/cases/*.nalu` and runs three tests over them. Each test runs the cases in
parallel (up to 8 threads), has a 60 s limit per process, collects **all** the failures and reports them
together at the end, with a diff (`-` expected, `+` got).

1. `cases_on_bash`
   - `nalu build x.nalu --target linux` and `--target macos`, then `bash -n` on both `.sh` files;
   - runs the script for this machine's target (`macos` on macOS, `linux` elsewhere) with `/bin/bash`, in an
     empty temporary folder, with the arguments from `x.args`, the variables from `x.env` and stdin closed;
   - compares stdout with `x.out` (or the target's variant), the exit code with `x.code` and the `x.err`
     lines with stderr.
2. `cases_on_bash_linux_target`: outside Linux, if a Bash 4.3+ is available (`NALU_BASH4`, or
   `/opt/homebrew/bin/bash`, or `/usr/local/bin/bash`), also runs the `linux` script and compares it the
   same way. This covers the code specific to that target (`declare -A`, `${x,,}`, `wait -n`, lists by
   reference). Without such a Bash, it prints a warning and only `bash -n` validates the target.
3. `cases_on_powershell`
   - `nalu build --target windows`;
   - checks the `.ps1`: pure ASCII with no BOM (a BOM only if the file has non-ASCII bytes, which today
     only a `native` block can cause, and then it is required); only CRLF line endings; and the minimal
     PowerShell 5.1 syntax checker (`nalu::ps::check_syntax`);
   - if PowerShell is available (`NALU_PWSH`, `pwsh` or `powershell`): `tests/ps_syntax.ps1` (the official
     parser, rejecting PowerShell 7 syntax) and, unless the case has `x.unix`, runs the script and compares
     stdout (with `\r\n` normalized to `\n`), the exit code and `x.err` as above. Without PowerShell, it
     prints a warning and only the checks above run.

Outside this folder, `tests/use/` (a script split across files with `use`, and two broken uses) is checked by `use_splits_a_script_across_files`; `tests/pacote/` is checked by `nalu_test_package_with_failure` (`nalu test` with a test
that fails on purpose and an unhandled error; expected output in `tests/pacote/saida.txt`, exit code 1), and
`examples/` by `examples_compile_for_all_three_targets` and `nalu_test_examples_passes`.

## The cases

| Case | Covers |
|---|---|
| `core` | `let`/`var`, `+= -= *= /= %=`, `if`/`else if`/`else`, `if` as an expression, `for` with `..`/`..=` and an empty range, `while` with `break`/`continue`, `for i, x`, `match` with ranges and `\|`, `print()` without arguments, `!` |
| `strings` | every `str` method, interpolation, `{{ }}`, `\{ \}`, `'raw'`, escapes, `"""` with dedent, `fmt`, `+` with non-text |
| `numbers` | truncation toward zero, `%` with the sign of the dividend, `1_000`, `0xFF`, the largest `int`, precision above 2^53, `to_int` (spaces, `+5`, overflow, decimal), `math.*`, comparisons |
| `division` | dividing by zero (`/` and `%`) fails on all three targets with the same message |
| `duration` | literals, arithmetic with units, text (`0s`, `250ms`, `1.5s`, `1m30s`, `1d2h`), `/ 1s`, comparison, `sum()` of durations, `time.sleep` |
| `lists` | every list method, expression and block lambdas (reading an outside variable), ordinal `sort`, stable `sort_by`, `slice`, index out of range, copy by value |
| `list_references` | on the linux target lists of scalars are passed by reference (`local -n`); the cases where that could go wrong: recursion, equal names in different functions, forwarding, temporaries, lambdas, `parallel`, methods, functions with `defer` |
| `index` | a list index out of range fails like any fallible operation: `catch`, function, `try`, `parallel`, lambda, struct, assignment; `&&` and `while` only evaluate the index when needed |
| `maps` | literal, `[k]`, `catch`, `has`, `remove`, insertion order (also after removing and reinserting), `for k, v`, copy, `int` keys |
| `structs` | literals with and without a name, fields with defaults, `..base`, methods and `self`, copy in `var`, lists of structs, list field, `find`/`map`/`sort_by` on structs, struct by value into a function |
| `nested_structs` | structs inside structs: nested fields read, written, rebuilt, passed, matched and sent through `parallel` |
| `nested_collections` | `[][]T` and `map[K][]T`: literals, items, loops, lambdas, methods, maps of lists |
| `nested_assignment` | assigning inside nested values: `grid[i][j] = v`, `xs[i].f = v`, `m[k][0] = v`, `rows[i].push(x)`; nested fields in lists of structs; `parallel for` over a `[][]T` |
| `nested_calls` | a mutating method on a nested value as the value of `let`/`var`, `=` and `return`, with and without `catch` |
| `list_fields` | lists of structs as fields: read, written in place, pushed, popped, passed, filtered, copied, sent through `parallel` |
| `named_argument_names` (unix) | `code`, `max` and `every` as ordinary names next to the named arguments that use those labels |
| `nested_single_item_lists` | regression for the Stela bug (a one-item `[][]string` flattened in PowerShell): one-item lists in returns, `split`, a list field, a list of structs, `for` over a return value |
| `enums` | qualified names, exhaustive `match` without the type name, `parse` with `catch`, text, equality |
| `match` | duration and bool patterns, inclusive ranges, an arm with a block, an arm with `fail` in a `match` expression |
| `match_patterns` | `match` with guards (`n if …`), bindings (`n =>`) and struct patterns, tested in order |
| `error_handling` | `fail` with a code, `catch` in its five forms, `try`/`catch`, propagation through functions, `err.code`, `catch { break }` inside a loop, a top-level `catch fail` → exit code and `error: …` |
| `top_level_failure` | an unhandled failure at the top level: `error: boom` on stderr and the `fail` code |
| `commands` (unix) | `{x}` as **one** argument (with a space, `;`, `$`, `*`), mixed words, `{list}`, `$( )` without the trailing line breaks, pipes, `value \| $` without an extra `\n`, `if $`, `if !$`, `catch ignore`, `catch { err.code }`, `>`, `>>`, `<`, `2>`, `2>&1`, a missing program (127) through a dynamic name, pipefail, the command's exit code |
| `command_env` (unix) | `NAME=value` before a program: literal and interpolated values, a secret, each stage of a pipeline, a condition, an empty value, nothing left in the script's environment, overriding `env.set`, failure messages |
| `echo` | portable `$ echo` (one line, arguments joined by spaces, no arguments = empty line), `>`/`>>` in UTF-8 without a BOM with `\n`, `$(echo …)` |
| `deps` | a missing program with a fixed name: nothing runs, code 127 |
| `params` | `--x v`, `--x=v`, bool, `--bool=false`, lists, `int[]`, enum, duration, path, positionals in `args` and `--` |
| `params_missing` | a required parameter is missing → code 2 and `use --help` |
| `params_invalid` | `--vezes 3x` → code 2 |
| `params_help` | `--help` prints the usage exactly as in the reference example |
| `dryrun` | `--dry-run`: commands and writes are shown (`[dry-run] …`) and not executed; captures and reads run; writes and `rm` inside `fs.temp_dir()` run |
| `verbose` | `--verbose`: `+ command` with the values; `log.debug` appears |
| `defer` | reverse order, arguments evaluated immediately, on handled failure, `exit()` inside a function runs the function's and the top level's `defer`s |
| `retry` | success on the 3rd attempt, failure after the last one with `catch { }`, default `every` |
| `parallel` | results in order with `max`, `catch` per task, aggregated error `parallel: …; …`, the task form, `parallel for` without a value |
| `parallel_structs` | `parallel for` that returns structs (list fields too), and `max: 0` (treated as 1) |
| `parallel_tempo` | tasks really run at the same time (also in PowerShell), `max` limits them, results follow the list order (timing based, uses 2 s tasks) |
| `functions` | defaults, named arguments out of order, recursion, `var` globals written in functions, lists and maps by value, `return` without a value |
| `scope` | shadowing in a function, a parameter named like a global, globals read, `let` in blocks, `push` on a global inside a function |
| `fs` | `write`/`append`/`read`/`size`/`exists`/`is_*`/`mkdir -p`/`ls`/`walk`/`glob`/`copy`/`move`/`replace`/`rm` (with guards)/`temp_file`/`sys.cd` |
| `path` | `name`/`stem`/`ext`/`parent`, a dotfile without an extension, `/`, `~`, properties of a missing path, `path.abs` |
| `tilde` | `~` in a command: unquoted it is the home folder, quoted it is text; a `path` with `~` is also expanded |
| `os` | `match os` and `if os == …` (output per target: `os.linux.out`, `os.macos.out`, `os.windows.out`) |
| `unicode` | `len` by code point, `upper`/`lower` outside ASCII, ordinal comparison, `chars`/`slice`/`index_of` with emoji, `sort` by code point |
| `quotes` | typographic quotes (`’ “ ” „ ‚ ‛`), `$HOME`, backtick, `$(x)`, `${y}`, `@(z)` in literals and in command arguments |
| `secret` | `secret` from `NALU_TOKEN`, `******` under `--dry-run --verbose`, the value never appears |
| `secret_stdin` (unix) | a `secret` only leaves through a command's input, without an extra `\n` |
| `log` | `info:`/`warning:`/`error:` on stderr; `debug` hidden without `--verbose` |
| `env` | `env.get`/`has`/`set`, `catch` on a missing variable, the value applies to child commands |
| `env_child` (unix) | `env.set` applies to child commands |
| `time` | `time.format` with the supported subset, `now`, `now_ms`, `since`, `since_ms`, `sleep`, `hour` |
| `system` | `sys.*`, `proc.exists`/`which` with `catch`, `dirs.*` |
| `data` | `json`, `hash`, `archive` and `fs.chmod`: the same result on the three targets |
| `regex` | `regex` and encoding: the same matches, groups and Base64 on every target |
| `exit` | `exit(300)` → code 44 (`% 256`), `defer` runs |
| `exit_zero` | `exit()` inside a loop → code 0 |
| `native` | `native { bash: …, powershell: … }` |

## Compile errors and warnings (`tests/errors`)

Every `tests/errors/*.nalu` file is a program that `nalu check` must diagnose. The first line names the
diagnostic:

- `// error: E0xx`: `nalu check` must fail (exit code 1) and stderr must contain the code;
- `// warning: W0xx`: `nalu check` must pass (exit code 0) and stderr must contain the code.

The file name is `<code>_<what it tests>.nalu` in English, for example `e001_unknown_field.nalu` or
`e090_planned_module.nalu`. The harness test is `compile_errors`.

## Open questions in the reference

Where the reference does not fix the behavior, the case either **avoids** the question or picks one reading
and records it here. The test names, flags and messages below are the ones used when the corpus was written.

1. **`"""` and the last line break.** "removes the common indentation and the first line break" does not say
   whether the line break before the closing `"""` (on a line with only spaces) stays in the text.
   `strings.nalu` closes the `"""` on the same line as the text so it does not depend on this. Suggestion:
   as in Swift, the line of the closing `"""` is not part of the text (no final `\n`).
2. **`"+5".to_int()`.** "fails if it is not a 64-bit integer" does not say whether the `+` sign is valid.
   Following the note in SPEC-003 ("Bash refuses; align"), `numbers.out` expects a **failure**.
3. **Text of durations with fractions and mixed units.** The reference shows `0s`, `250ms`, `1.5s`, `1m30s`,
   `1d2h`. The cases assume `2.5s` for 2500 ms (like `1.5s`) and avoid `90500 ms` (`1m30.5s`?) and negatives.
4. **`fs.walk`: dots and prefix.** "paths relative to dir as it was passed" was read as "each path starts
   with `dir` as it was passed" (like `find dir`), with `/` on Windows too (SPEC-003 note), and it includes
   files that start with `.` (`find` includes them). `fs.nalu` depends on this.
5. **`fs.glob` in order.** Assumes a sorted result (byte order) and that `.*` does not return `.` or `..`.
6. **Order of `parallel` messages.** Assumes the order of the list/tasks (`parallel: falhou 7; falhou 9`),
   not the order of completion.
7. **Unhandled error inside `test`.** The reference only shows `FAILED  name: assert failed: …`. For an
   unhandled error, `tests/pacote/saida.txt` assumes `FAILED  name: <error message>`; summary
   `N test(s), M failure(s)`.
8. **Message of a failed command.** "the message says which command failed and with what code": the cases
   only check `error:` on stderr and the exit code; SPEC-002 suggests ``command `sh` failed with code 5``.
9. **Text of `[dry-run]` for stdlib writes** (`[dry-run] fs.write out.txt`?) is not fixed; `dryrun` only
   checks the one for commands (`[dry-run] touch criado.txt`).
10. **`--verbose` shows the values without quotes** (`+ echo a b` for `echo {x}` with `x = "a b"`): this is
    the reading of "the backend replaces `{…}` with the values".
11. **`~` and `dirs.home()`.** `path.nalu` assumes that `("~/algo": path).parent == dirs.home()` (the
    properties expand `~`, and `dirs.home()` has no trailing `/`).
12. **Strings inside `{ }` in an interpolation** (`"{t.split(",").len()}"`,
    `"[{fs.glob("nada*").join(",")}]"`): the reference says `{expr}` accepts any expression; the `strings`,
    `fs` and `commands` cases use quotes inside the braces.
13. **`let` with the same name in sibling blocks** (`if true { let y = 5 }` and then `let y = 6` at the
    top): `scope.nalu` assumes it is allowed ("redeclaring in the **same** block is an error").
14. **`param` bool with default `true`** (`param limpar_cache: bool = true` and `--limpar-cache=false`):
    `params.nalu` assumes it is allowed.
15. **`print` of an enum and of a bool as a direct argument** (`print(a)` with `a: Ambiente`) prints the case
    name / `true`: read from "`any` = any scalar that becomes text".
16. **`exit(300)`**: the reference says nothing about codes outside 0..255; `exit.code` expects 44 (Bash
    wraps around; SPEC-003 tells PowerShell to apply `% 256`).
17. **`<` in a pipeline.** The grammar puts redirections at the end of the command; in PowerShell, `<` feeds
    the **first** stage and `>`/`2>`/`2>&1` the last one. No case uses `<` with `|`.
18. **`dirs.cache`/`dirs.data` on Windows** are not in the reference; the PowerShell backend follows the
    `dirs` crate (`%LOCALAPPDATA%\app` and `%APPDATA%\app`). `system.nalu` only checks the last component.
19. **`nalu test [folder]`**: the reference says "joins the `.nalu` files of the folder", but `examples/` has
    independent scripts with their own `param`s, which do not compile together. The CLI runs **packages**:
    `x_test.nalu` with `x.nalu`, or a file with its own `test` blocks (see SPEC-004, Report).

Left out of the corpus because they depend on the environment: `ui.*` (needs a terminal; without one
`confirm` gives `false` and `ask` gives the default) and `http.*` (network).
