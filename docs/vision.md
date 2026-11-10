# Vision

> **Nalu**: one wave, every shell. *Nalu* is Hawaiian for the wave that reaches the shore.

## In one sentence

Nalu is a small, typed language for setup, build, deploy and install scripts that compiles one file into a native script for each system (Bash for Linux and macOS, PowerShell for Windows), so nothing has to be installed where the scripts run.

## The problem

A script that has to run on Linux, macOS and Windows usually becomes three files: `install.sh`, `install.ps1` and often an `install.cmd`. They drift apart, because every fix has to be made three times in three languages.

Each of those languages also makes it easy to write a script that is wrong without looking wrong:

- **Quoting.** In Bash, an unquoted `$dir` splits on spaces and expands globs; an empty variable turns `rm -rf "$dir/"` into `rm -rf /`. In Windows PowerShell 5.1, arguments to native programs that contain quotes or are empty are passed incorrectly.
- **Silent failures.** A failed command does not stop a Bash script unless you use `set -e`, which has well-known gaps (inside functions called from `if`, in pipelines, in `$( )`). PowerShell's `$ErrorActionPreference` does not apply to native programs' exit codes in 5.1.
- **No types and few tests.** A typo in a variable name is an empty string. Testing means a separate framework per shell, so most scripts have none.

The usual escape is Python, Node or a compiled Go binary. Each of those asks the target machine for something first: an interpreter of the right version, or a binary built, signed and downloaded for each system and architecture.

## What Nalu does

A complete script, `install.nalu`:

```nalu
/// where to install
param dest: path = "~/.local/bin"

let tag = $(git describe --tags) catch "v0.0.0"

$ cargo build --release
fs.mkdir(dest)
fs.copy("target/release/app", dest / "app")
fs.chmod(dest / "app", "+x")
log.info("app {tag} installed in {dest}")
```

```text
install.nalu ──► nalu build ──┬─► dist/install.linux.sh     Bash 4.3+
                              ├─► dist/install.macos.sh     the Bash 3.2 that ships with macOS
                              ├─► dist/install.windows.ps1  Windows PowerShell 5.1+ (also 7)
                              └─► dist/install.windows.cmd  launcher for the .ps1
```

The compiler checks it (types, names, which operations can fail) and writes a plain script for each system. The output is the only thing that has to exist on the target: no runtime, no interpreter, no download.

Every generated script:

- takes `--help` (written from the `///` comments on its `param`s), `--dry-run` and `--verbose`;
- checks that the programs it calls exist before it changes anything;
- quotes every argument and checks every failure where it happens;
- is readable, with `# line N` comments that point back to the source, so you can audit what you pipe into `bash`.

| Target | Shell | Notes |
|---|---|---|
| `linux` | Bash 4.3+ | Native associative arrays and `wait -n`; written for GNU coreutils and BusyBox. |
| `macos` | Bash 3.2 | No Bash 4 features (`declare -A`, `wait -n`, `${x,,}`); BSD tools. |
| `windows` | Windows PowerShell 5.1+ | `.ps1` saved as ASCII with CRLF (it also runs with `irm … | iex`), plus a `.cmd` launcher; also runs on PowerShell 7. `parallel` runs its tasks at the same time in a runspace pool. |

## Who it is for

- Projects that ship an installer for `curl | bash` and for PowerShell, and want one source for both.
- CI pipelines that run the same steps on Linux, macOS and Windows runners.
- DevOps and platform teams with internal tools that have to work on everyone's laptop.
- AI agents that generate scripts: the compiler checks them, explains them and formats them, so a model can write, check and fix its own code before anyone runs it.

## Principles

1. **Fail loud by default; recover on purpose.** Any command or call that fails stops the script (or makes the current function fail) with a precise message and exit code. To do anything else, you write `catch`: `fs.read(p) catch ""`, `$ git pull catch fail "no network?"`, `$ docker logout catch ignore`. There is no `try` on every line and no `throws` in signatures, because propagation is always the same; recovery is the decision worth writing down. Since a single line does not show whether it can fail, `nalu check --explain` lists every line that can, and what happens if it does.

2. **One way to do each thing.** One loop (`for`), one way to handle a failure (`catch`), one way to choose (`match`), one way to turn a value into text (`"{x}"`), one way to read a map (`m[k]`), one format (`nalu fmt`). Fewer choices mean scripts that look alike, review faster, and are easier for a model to get right.

3. **Familiar syntax, no ceremony.** `fn` and `->`, `let` and `var`, `match` with `|` and `_`, `if` as an expression, `{x}` interpolation, types written like Go (`[]str`, `map[str]int`). No imports, no `main`, no semicolons, no `new`.

4. **Honest on every target.** Each system gets code written for it: Linux gets Bash 4.3 features, macOS keeps its stock Bash 3.2, and Windows PowerShell 5.1 gets the known traps handled by the compiler (native programs run through `System.Diagnostics.Process` with correct argument escaping, files written as UTF-8 without BOM, ordinal text comparison). When something cannot behave the same everywhere, `nalu check` says so instead of producing a script that differs quietly.

5. **One tree of decisions.** The front end lowers every script to one flat, typed intermediate representation in which each operation that can fail carries its own failure handling. The Bash and PowerShell back ends only translate it, so the targets agree by construction rather than by testing alone. The runtime helpers are real `.sh` and `.ps1` files with their own tests, and a script includes only the helpers it uses.

6. **No magic in the shell.** No `eval` of data, no `set -e`, no word splitting or globbing of values. A `secret` is never printed, logged or passed in a command's arguments; it reaches a program only through standard input or as an HTTP `bearer:` token.

7. **Portable batteries.** `fs`, `env`, `http`, `json`, `hash`, `archive`, `time`, `ui`, `log` and the rest behave the same on all three targets: ordinal text, UTF-8, `\n` line endings. JSON is parsed by the script itself, so there is no `jq` to install.

8. **Built for people and machines.** The parser recovers and reports several errors per run. Diagnostics have stable codes (`E006`, `W010`), most come with a `help:` line that says how to fix them, and `nalu check --json` gives the same to editors and agents. Warnings are kept for things that are not bugs, such as an unused variable. `nalu doc` documents any name, and [`llms.txt`](../llms.txt) fits the language on one page.

9. **A fast compiler with zero dependencies.** Rust and its standard library only. Compiling a script takes milliseconds, which makes checking on every keystroke practical.

## Non-goals

- **General-purpose programming.** No GUIs, no numeric work, no `float`. Nalu is for scripts that drive other programs and files.
- **A runtime or VM on the target.** The output is the script and nothing else.
- **Covering all of Bash or PowerShell.** For the rare thing the standard library cannot do portably, `native { bash: "…", powershell: "…" }` inserts raw shell code for each target.
- **Classes, inheritance, user-defined generics, macros.** Structs, enums and functions are enough for scripts, and every extra concept is one more thing to read in a review.

## How Nalu compares

| | Bash and PowerShell by hand | Python or Node | A Go binary | Nalu |
|---|---|---|---|---|
| Needed on the target | nothing | an interpreter, at the right version | the right binary for the OS and CPU | nothing |
| Sources for three systems | two or three | one | one | one |
| Quoting | by hand, per shell | library calls | library calls | `{x}` is always one argument |
| A failed command | continues unless you remember to check | depends on the API; often continues unless you check | returns an error you must check | stops the script unless you write `catch` |
| Types and tests | none built in | optional | yes | yes, plus `--dry-run` |
| What the user can read before running it | the script | the script and its dependencies | nothing: it is compiled | the generated script |

**Writing Bash and PowerShell by hand** costs nothing to run, and for a short script for one system it is often the right choice. It stops scaling when the same logic has to live in two languages with different quoting, error and encoding rules, and nothing checks either of them before they run.

**Python, Node and Go** are better languages for programs. For an installer or a CI step, they move the problem: the target needs the interpreter first (no Windows machine ships one), or you build, sign and host a binary for every system and architecture. Running other programs from them also takes more ceremony than in a shell.

**Nalu** keeps the shell's strengths (running programs is one line, and the output is a plain script) and adds what scripts usually lack: one source, a compiler that checks types and names, explicit failure handling, tests, and a dry run.

Pick something else when the task is really a program, such as a service, a data pipeline or anything with a GUI, or when it needs features Nalu does not have yet, such as floating-point numbers or a package manager module (see the [limitations](language.md#limitations)).

## Read next

- [Getting started](getting-started.md): install, write and build your first script.
- [Recipes](recipes.md): installers, API calls, backups, CI.
- [Language reference](language.md) and [standard library](stdlib.md).
- [Architecture](architecture.md): the pipeline, the IR and the runtime.
- [Decisions](decisions.md): why things are the way they are.
