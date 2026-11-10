# Getting started

Nalu is a small, typed language for setup, build, deploy and install scripts. You write one `.nalu` file; `nalu` checks it and writes Bash for Linux and macOS and PowerShell for Windows. The machines that run those scripts need nothing installed.

This guide takes about 15 minutes. You will write a script, run it, build it for the three systems, handle failures, and set up your editor.

## Install the compiler

Download the binary for your system from the [latest release](https://github.com/JonathanSantos/nalu/releases/latest) (macOS, Linux and Windows, with a `checksums.txt`) and put it in a folder on your `PATH`; the [README](../README.md#install) has the commands.

Or build it yourself. Nalu is written in Rust with no dependencies. From a clone of the repository, with a Rust toolchain installed:

```bash
cargo build --release
```

The binary is `target/release/nalu`. Copy it to a folder on your `PATH`, or call it by its path:

```bash
cp target/release/nalu ~/.local/bin/
nalu --help
```

You only need `nalu` on the machine where you write scripts. The scripts it writes run on stock Bash (4.3+ on Linux, the 3.2 that ships with macOS) and Windows PowerShell 5.1+.

## Your first script

Create `hello.nalu`:

```nalu
/// who to greet
param name: str = "world"

let user = env.get("USER") catch env.get("USERNAME") catch "someone"
print("hello, {name}! I am {user}")
$ git --version
```

Run it:

```console
$ nalu run hello.nalu -- --name Ana
hello, Ana! I am sam
git version 2.50.1 (Apple Git-155)
```

`nalu run` compiles the script for the system you are on and runs it. Everything after `--` goes to the script. A few things happened without you asking:

- `param` became the `--name` option, and the `///` comment above it became its help text.
- `{name}` inside double quotes is interpolation. In a `$` command, `{x}` is always exactly one argument, spaces and all.
- `env.get("USER")` fails when the variable is not set, so `catch` tries the next one (Linux and macOS set `USER`, Windows sets `USERNAME`).
- `$ git --version` runs a program. Before the first line runs, the script checks that `git` exists; if it does not, it stops with `error: this script needs: git` and exit code 127.

Every script gets `--help` for free:

```console
$ nalu run hello.nalu -- --help
usage: hello [options] [args…]

  --name <str>  who to greet (default: world)
  --dry-run     show what would happen, without running commands or changing files
  --verbose     print each command before running it
  -h, --help    show this help
```

## Build for Linux, macOS and Windows

```console
$ nalu build hello.nalu
$ ls dist
hello.linux.sh  hello.macos.sh  hello.windows.cmd  hello.windows.ps1
```

| File | Runs on |
|---|---|
| `hello.linux.sh` | Bash 4.3 or newer |
| `hello.macos.sh` | the Bash 3.2 that comes with macOS, with BSD tools |
| `hello.windows.ps1` | Windows PowerShell 5.1 or newer (also PowerShell 7) |
| `hello.windows.cmd` | a small launcher for the `.ps1`, so a double click or `cmd.exe` works whatever PowerShell's execution policy is |

Run them like any other script:

```console
$ ./dist/hello.linux.sh --name Ana
C:\> dist\hello.windows.cmd --name Ana
```

Use `--target linux,windows` to build only some of them and `-o out` to change the folder. To read what Nalu generated without writing files, use `nalu emit hello.nalu --target windows`. The output is plain, readable shell, with a `# line N` comment that points back to your source.

Every generated script also takes:

- `--dry-run`: shows the commands and file changes it would make, without making them. Reads, `$( )` captures and anything inside a folder from `fs.temp_dir()` still run, so the rest of the script has real data.
- `--verbose`: prints each command before running it and turns on `log.debug`.

## When something fails

Nalu has one rule for errors: **any operation that can fail stops the script with a clear message**, unless you say otherwise. That covers a command that exits with a nonzero code, a file that does not exist, `"abc".to_int()`, and `xs[9]` on a list of three. If `report.nalu` starts with `let text = fs.read("settings.txt")` and there is no such file:

```console
$ nalu run report.nalu
error: fs.read: 'settings.txt' does not exist
$ echo $?
1
```

A failed command stops the script with that command's own exit code. Inside a function, the failure makes the function fail, and its caller faces the same question.

To do something else, write `catch` after the operation:

```nalu
fn deploy(stage: str) {
    $ make deploy STAGE={stage}
    log.info("{stage} is up to date")
}

let config = fs.read("config.txt") catch ""             // a default value
let port = env.get("PORT") catch fail "set PORT first"  // a better message
$ git pull catch ignore                                 // keep going
let version = fs.read("VERSION") catch {                // a block, with the error in `err`
    log.warn("no VERSION file ({err}), using 0.0.0")
    "0.0.0"
}
print("port {port}, version {version.trim()}, {config.lines().len()} lines of config")

deploy("staging") catch {
    log.error("staging failed: {err}")
    exit(err.code)
}
```

- `catch value` uses a value instead. `catch fail "…"` fails with your message. `catch ignore` moves on (only where no value is needed). `catch { … }` runs a block; its last line is the value.
- Inside the handler, `err` is the failure: `{err}` or `err.message` is the message, `err.code` the exit code.
- `fail "message"` fails on purpose; `fail "message", code: 3` picks the exit code.
- `try { … } catch …` puts several lines under one handler: the first failure jumps to it.

There is no `try` on every line and no `throws` in signatures. The trade-off is that you cannot always tell, from one line, whether it can fail. The compiler can:

```console
$ nalu check --explain failures.nalu
failures.nalu:2:5: `$ make` can fail → `deploy` fails
failures.nalu:7:12: `catch fail` can fail → the script stops
2 places can fail without `catch`
functions that can fail: deploy
```

`catch fail` is listed because it fails on purpose. The call to `deploy` on line 15 is not listed: its failure is handled. The VS Code extension shows the same information as a ⚡ at the end of each line.

## Functions, types and lists

```nalu
enum Stage { dev, prod }

struct Service {
    name: str
    port: int

    fn url(stage: Stage) -> str {
        let host = match stage {
            dev  => "localhost"
            prod => "example.com"
        }
        return "http://{host}:{self.port}"
    }
}

let services = [
    Service { name: "api", port: 8080 },
    Service { name: "web", port: 3000 },
]
var replicas: map[str]int = { "api": 3 }
replicas["web"] = 2

for s in services {
    let n = replicas[s.name] catch 1
    print("{s.name} at {s.url(Stage.dev)}, {n} replicas")
}

let busy = services.filter(s => s.port < 5000).map(s => s.name)
let timeout = 1m30s
print("low ports: {busy.join(", ")}; timeout {timeout} = {timeout / 1s} seconds")
```

```text
api at http://localhost:8080, 3 replicas
web at http://localhost:3000, 2 replicas
low ports: web; timeout 1m30s = 90 seconds
```

- `let` names a value that cannot change; `var` one that can. Types are inferred inside a script; function signatures are written out.
- The types are `str`, `int`, `bool`, `path`, `duration` (`500ms`, `30s`, `1m30s`, `7d`), `secret`, lists `[]str`, maps `map[str]int`, plus your own `struct` and `enum`.
- There is no `null`. Something that might be missing fails instead, and `catch` supplies the fallback: `m[k]` fails for a missing key, `xs[i]` for an index out of range.
- `if` and `match` are expressions. Inside `match`, enum cases are written without the type name.
- Turn any value into text with interpolation: `"{n}"`. Check for empty with `s == ""` or `xs.len() == 0`.
- Paths join with `/` and use the right separator on each system: `dirs.home() / ".config" / "app"`. `p.name`, `p.stem`, `p.ext` and `p.parent` look only at the text; `fs.exists(p)` and `fs.size(p)` look at the disk.

## Running commands

```nalu
/// image to build
param image: str = "my-app"

let commit = $(git rev-parse --short HEAD) catch "unknown"
$ docker build -t {image}:{commit} .
$ docker image ls {image} > images.txt 2>&1
let manifest = fs.read("deploy.yaml")
manifest | $ kubectl apply -f -
if $ git diff --quiet {
    print("the working tree is clean")
}
$ git push catch fail "could not push; are you online?"
```

- A line that starts with `$ ` runs a program. `$( … )` captures its standard output, without trailing newlines. `>`, `>>` and `2>&1` redirect as in a shell.
- `value | $ cmd` sends a value to the command's standard input. This is the only way a `secret` reaches a command, so tokens never appear in process arguments.
- `if $ cmd { … }` uses the exit code as the condition.
- Unquoted words are literal text: no globbing, no word splitting, and `$HOME` stays `$HOME`. Use `{x}` for values; a list `{xs}` on its own becomes one argument per item.
- An unquoted ` catch ` ends the command and starts its handler.

Prefer the standard library over shell tools when there is one (`fs.copy` over `cp`, `json.get` over `jq`): it behaves the same on all three systems. `nalu check` warns when a program you call does not exist on one of them, such as `grep` on Windows.

## Tests

Put `test` blocks next to your functions and run them with `nalu test`:

```nalu
fn readable(bytes: int) -> str {
    if bytes < 1024 {
        return "{bytes} B"
    }
    return "{bytes / 1024} KB"
}

test "small sizes stay in bytes" {
    assert readable(512) == "512 B"
}

test "kilobytes are rounded down" {
    assert readable(1536) == "1 KB"
}

test "a missing file fails" {
    assert_fails(fs.size("no-such-file"))
}

for file in fs.glob("*.log") {
    print(file.name, readable(fs.size(file)))
}
```

With this file saved as `sizes.nalu`:

```console
$ nalu test
ok      small sizes stay in bytes
ok      kilobytes are rounded down
ok      a missing file fails
3 test(s), 0 failure(s)
```

`nalu test` runs the tests of every `.nalu` file in the current folder (or in the folder you name, without going into subfolders) and exits with code 1 if any fails. It loads the functions and types but does not run the script's top-level lines, so the loop at the end does not run. `nalu run` and `nalu build` ignore the tests.

When a script grows, move its tests to `sizes_test.nalu` in the same folder: a `_test.nalu` file sees the functions and types of the script with the same name, as in [`release_test.nalu`](../examples/release_test.nalu).

## Set up your editor

The VS Code extension in [`editors/vscode`](../editors/vscode/README.md) has no dependencies and asks the compiler for everything: live checks as you type, a ⚡ on each line that can fail, types and docs on hover, completion by type, signature help, go to definition, and format on save. To install it from the repository root:

```bash
ln -s "$(pwd)/editors/vscode" ~/.vscode/extensions/nalu-vscode
```

Then run **Developer: Reload Window**. The extension uses the `nalu.path` setting if you set it, then `target/release/nalu` in the open folder, then `nalu` on your `PATH`.

From the terminal:

```console
$ nalu fmt .                   # format every .nalu file under this folder
$ nalu fmt --check .           # change nothing; list the files that need formatting (for CI)
$ nalu doc fs.copy             # the docs for any module, function, type or keyword
```

`nalu fmt` fixes indentation and spacing and keeps your line breaks and comments.

## Where to go next

- [Recipes](recipes.md): an installer for `curl | bash` and Windows, API calls with a secret token, JSON, backups with checksums, a CI step for three systems.
- [Examples](../examples/): complete scripts such as [`deploy.nalu`](../examples/deploy.nalu) and [`installer.nalu`](../examples/installer.nalu).
- [Language reference](language.md): `parallel`, `retry`, `defer`, `native`, `embed` and the rest.
- [Standard library](stdlib.md): every module and method. `nalu doc <name>` prints the same entries.
- [Vision](vision.md): why Nalu exists and what it will not do.
- [`llms.txt`](../llms.txt): the language on one page, for an AI assistant's context.
