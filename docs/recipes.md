# Recipes

Common setup, build, deploy and install tasks, ready to copy and adapt. Every script here compiles with the current compiler. New to Nalu? Start with [getting started](getting-started.md).

- [Installer for `curl | bash` and Windows](#installer-for-curl--bash-and-windows)
- [Call an API with a secret token](#call-an-api-with-a-secret-token)
- [Read a JSON file](#read-a-json-file)
- [Back up folders with a checksum manifest](#back-up-folders-with-a-checksum-manifest)
- [One CI step for three systems](#one-ci-step-for-three-systems)
- [Wait for a service to be healthy](#wait-for-a-service-to-be-healthy)
- [Run independent steps in parallel](#run-independent-steps-in-parallel)
- [Install a package on any system](#install-a-package-on-any-system)
- [Require a minimum version of a tool](#require-a-minimum-version-of-a-tool)
- [Delete old files, after asking](#delete-old-files-after-asking)
- [Check a script before you run it](#check-a-script-before-you-run-it)

## Installer for `curl | bash` and Windows

**Goal:** one source for your project's `install.sh` and `install.ps1`: download a release, check its SHA-256, and install the binary.

```nalu
// install.nalu: installs a release of `tool` from GitHub, after checking its SHA-256.

/// version to install, e.g. v1.2.3; empty installs the latest
param version: str = ""
/// where to install
param dest: path = "~/.local/bin"

let repo = "owner/tool"
let ext = if os == "windows" { "zip" } else { "tar.gz" }
let package = "tool-{os}-{sys.arch()}.{ext}"
let base = if version == "" {
    "https://github.com/{repo}/releases/latest/download"
} else {
    "https://github.com/{repo}/releases/download/{version}"
}

let tmp = fs.temp_dir()
defer fs.rm(tmp)

log.info("downloading {package}")
http.download("{base}/{package}", tmp / package)

// The release publishes `<package>.sha256` in sha256sum format: "<hash>  <file>".
let expected = http.get("{base}/{package}.sha256").trim().split(" ")[0]
if hash.sha256_file(tmp / package) != expected {
    fail "the SHA-256 of {package} does not match: the download may be corrupted"
}

if os == "windows" {
    archive.zip_extract(tmp / package, tmp / "x")
} else {
    archive.tar_extract(tmp / package, tmp / "x")
}

let exe = if os == "windows" { "tool.exe" } else { "tool" }
fs.mkdir(dest)
fs.copy(tmp / "x" / exe, dest / exe)
fs.chmod(dest / exe, "+x")
log.info("installed {dest / exe}")
```

Build it and publish the scripts next to your release or on your website:

```bash
nalu build install.nalu -o dist
# upload dist/install.linux.sh, dist/install.macos.sh and dist/install.windows.ps1
```

Your users install with:

```bash
curl -fsSL https://example.com/install.linux.sh | bash                          # Linux
curl -fsSL https://example.com/install.macos.sh | bash -s -- --version v1.2.3   # macOS, with options
```

```powershell
powershell -c "irm https://example.com/install.windows.ps1 | iex"      # Windows
```

With options on Windows, download the file and run it:

```powershell
irm https://example.com/install.windows.ps1 -OutFile install.ps1
powershell -ExecutionPolicy Bypass -File install.ps1 --version v1.2.3
```

- The release assets follow a naming convention: `tool-<os>-<arch>.tar.gz` (`.zip` on Windows), where `<arch>` is what `sys.arch()` returns (`x64`, `arm64`), with the binary at the archive's root and a `.sha256` file next to each archive.
- `os` is a compile-time constant: the Linux script contains only the `tar` branch, the Windows script only the `zip` branch.
- `defer fs.rm(tmp)` removes the temporary folder when the script ends, even after a failure or Ctrl+C.
- `--dry-run` still downloads and checks the archive, because those writes go to the temporary folder, and shows the changes it would make to `dest` without making them.
- The generated `.ps1` is plain ASCII, so `irm … | iex` reads it correctly. Run it inside `powershell -c "…"`, as above: the script ends with `exit` on a failure or an `exit(n)`, and in its own process that only ends the installer, not your window.
- For private repositories or the GitHub API (a token, finding assets by name), see [`examples/installer.nalu`](../examples/installer.nalu).

## Call an API with a secret token

**Goal:** post a JSON message to an HTTP API, authenticated with a bearer token that never leaks.

```nalu
// notify.nalu: posts a message to a chat API with a bearer token.

/// API token
param token: secret
/// channel to post to
param channel: str = "deploys"

let version = fs.read("VERSION").trim()
let text = "version {version} is live"
let body = """{{"channel": {json.quote(channel)}, "text": {json.quote(text)}}}"""
let reply = http.post("https://chat.example.com/api/messages", body, bearer: token)
let id = json.get(reply, "id") catch "?"
print("message {id} sent")
```

With `--dry-run`, nothing is sent:

```console
$ NALU_TOKEN=… ./notify.linux.sh --channel releases --dry-run
[dry-run] http.post https://chat.example.com/api/messages
message ? sent
```

- A `secret` param is read from `--token` or from the `NALU_TOKEN` environment variable, and is required (`error: --token is required (or set NALU_TOKEN)`). Prefer the variable: a `--token` value is visible in the script's own command line.
- `bearer:` becomes an `Authorization: Bearer …` header. A secret is never printed, logged or passed in a command's arguments, and cannot be interpolated into text.
- `json.quote(x)` returns `x` as a JSON string, with quotes and escapes: the safe way to put a value inside JSON. In `"""…"""` you can write `"` freely; `{{` and `}}` are literal braces. For fixed JSON with no values, use single quotes: `'{"ok": true}'`.
- `http.post` sends `application/json` unless you pass `content_type:`, and fails on a network error or a status of 400 or more. With `--dry-run` it is only shown and returns `""`, which is why `id` fell back to `?` above.

## Read a JSON file

**Goal:** read values from a JSON config without `jq`.

```nalu
// pkg.nalu: reads a package.json and reports its name, version and scripts.

/// the package.json to read
param file: path = "package.json"

let manifest = fs.read(file)
let name = json.get(manifest, "name")
let version = json.get(manifest, "version") catch "0.0.0"
let scripts = json.keys(manifest, "scripts") catch []
print("{name}@{version}: {scripts.join(", ")}")
if json.has(manifest, "dependencies.left-pad") {
    log.warn("still depends on left-pad")
}
let first_file = json.get(manifest, "files[0]") catch "(none)"
print("first published file: {first_file}")
```

With this `package.json`:

```json
{
  "name": "demo",
  "version": "1.2.0",
  "files": ["dist", "README.md"],
  "scripts": { "build": "tsc", "test": "vitest" },
  "dependencies": { "left-pad": "^1.3.0" }
}
```

```text
demo@1.2.0: build, test
warning: still depends on left-pad
first published file: dist
```

- Paths use `.` and indexes: `"a.b.0"` and `"a.b[0]"` are the same; `""` is the whole document.
- Text comes back without quotes. Numbers, booleans, lists and objects come back as JSON text, so `json.get(doc, "files")` gives `["dist", "README.md"]`; `json.get_list` gives the items one by one.
- `json.get` fails when the path is missing or `null`; `json.has` checks without failing.
- The script parses JSON itself, so it works on machines without `jq` or Python.

## Back up folders with a checksum manifest

**Goal:** archive some folders into a dated folder, with a `SHA256SUMS` file to verify them later.

```nalu
// backup.nalu: archives folders and writes a SHA256SUMS manifest next to the archives.

/// folders to back up (comma-separated)
param folders: []str = ["docs", "config"]
/// where to keep the backups
param dest: path = "~/backups"

let tmp = fs.temp_dir()
defer fs.rm(tmp)

// Build everything in a temporary folder. With --dry-run these writes still happen,
// so the checksums are real; only the copy into `dest` is shown.
var sums: []str = []
for folder in folders {
    let dir: path = folder
    let name = "{dir.name}.tar.gz"
    archive.tar_create(tmp / name, dir)
    sums.push("{hash.sha256_file(tmp / name)}  {name}")
}
fs.write(tmp / "SHA256SUMS", sums.join("\n") + "\n")

let final = dest / time.format("%Y%m%d-%H%M%S")
fs.mkdir(dest)
fs.copy(tmp, final)
log.info("{folders.len()} folders saved in {final}")
```

```bash
./backup.linux.sh --folders src,notes --dest /mnt/backups
cd /mnt/backups/20261109-143000 && sha256sum -c SHA256SUMS   # shasum -a 256 -c on macOS
```

- A `[]str` param takes a comma-separated list: `--folders src,notes`.
- `let dir: path = folder` turns text into a path, so `.name` gives the last component (`notes` for `work/notes`).
- `SHA256SUMS` uses the format that `sha256sum -c` and `shasum -a 256 -c` read.
- `fs.copy` of a folder to a new path creates it with the folder's contents. To compress folders in parallel, see [`examples/backup.nalu`](../examples/backup.nalu).

## One CI step for three systems

**Goal:** replace three different `run:` blocks with one script that builds, tests and stores a checksummed artifact.

```nalu
// ci.nalu: builds and tests the project, then keeps the binary and its checksum.

/// where to put the artifacts
param out: path = "artifacts"

$ cargo build --release
$ cargo test --release

let ext = if os == "windows" { ".exe" } else { "" }
let name = "app-{os}-{sys.arch()}{ext}"
fs.mkdir(out)
fs.copy("target/release/app{ext}", out / name)
fs.write(out / "{name}.sha256", "{hash.sha256_file(out / name)}  {name}\n")
if sys.is_ci() {
    log.info("artifact {name} is ready")
}
```

Build the scripts once with `nalu build ci.nalu -o scripts`, commit the `scripts` folder, and call the right one on each runner:

```yaml
# .github/workflows/ci.yml
on: [push, pull_request]
jobs:
  build:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - if: runner.os == 'Linux'
        run: ./scripts/ci.linux.sh
      - if: runner.os == 'macOS'
        run: ./scripts/ci.macos.sh
      - if: runner.os == 'Windows'
        run: .\scripts\ci.windows.cmd
```

- The `.sh` files are written as executables, and the `.cmd` launcher runs the `.ps1` with Windows PowerShell 5.1 whatever the execution policy is.
- A failing `cargo` command stops the script with cargo's own exit code, which fails the CI step.
- `sys.is_ci()` is true when the `CI` environment variable is set, as it is on GitHub Actions and most CI services.

## Wait for a service to be healthy

**Goal:** after starting a service, wait until its health check answers 200, or give up.

```nalu
// wait.nalu: waits until a service answers 200 on its health check.

/// the health check URL
param url: str = "http://localhost:8080/health"
/// how many times to try, 3 seconds apart
param attempts: int = 20

fn healthy(u: str) -> bool {
    return (http.status(u, timeout: 5s) catch 0) == 200
}

retry(attempts, every: 3s) {
    if !healthy(url) {
        fail "{url} is not answering yet"
    }
}
log.info("{url} is up")
```

- `retry` runs the block again each time it fails. After the last attempt, it fails with the last error, which stops the script here; add `catch` after the block to do something else.
- `http.status` fails on a network error (the service is not listening yet); `catch 0` turns that into "not healthy".
- Durations are typed: `5s`, `3s`, `1m30s`. A `duration` param takes the same syntax on the command line.

## Run independent steps in parallel

**Goal:** run checks that do not depend on each other at the same time, and see every failure, not just the first.

```nalu
// checks.nalu: runs independent checks at the same time and reports every failure.

let tasks = ["lint", "test", "docs"]

parallel(max: 2) for task in tasks {
    $ make {task}
}
log.info("all {tasks.len()} checks passed")
```

- `parallel for` waits for every task. If any fail, it fails with all their messages, in task order: ``error: parallel: command `make lint` failed with exit code 2; command `make docs` failed with exit code 2``.
- `max: 2` limits how many run at once; without it, the limit is the number of CPUs.
- The body cannot change variables outside it. To collect results, make the body's last line a value: `let sizes = parallel for f in files { fs.size(f) }` gives a list in the original order.
- On Windows the tasks run in a pool of runspaces; tasks that call `sys.cd` or `env.set` run one at a time.

## Install a package on any system

**Goal:** make sure a tool is installed, using each system's package manager.

```nalu
// install_tools.nalu: installs git and jq with the system's package manager.

if !proc.exists("git") {
    // the package is called `git` everywhere except in winget
    let name = if pkg.manager() == "winget" { "Git.Git" } else { "git" }
    pkg.install([name])
}
if !proc.exists("jq") {
    pkg.install([if pkg.manager() == "winget" { "jqlang.jq" } else { "jq" }])
}
$ git --version
$ jq --version
```

- `pkg.manager()` names the manager found on the machine: `apt-get`, `dnf`, `yum`, `pacman`, `zypper` or `apk` on Linux, `brew` on macOS, `winget`, `choco` or `scoop` on Windows, and `""` when there is none.
- `pkg.install` answers yes to the manager's questions and, on Linux, runs it with `sudo` when the script isn't root. Package names are the manager's own, so pick them by `pkg.manager()` where they differ.
- A script normally refuses to start when a program it calls is missing (`error: this script needs: jq`). A program the script asks about with `proc.exists` is the exception, so the two commands at the end run after the install.
- With `--dry-run`, the script shows `[dry-run] pkg.install git` and installs nothing.

## Require a minimum version of a tool

**Goal:** stop early, with a clear message, when an installed tool is too old.

```nalu
// need_git.nalu: requires git 2.30 or newer.

let need = "2.30"
let have = regex.find($(git --version), '\d+\.\d+\.\d+') catch fail "no version in the output of `git --version`"
if semver.compare(have, need) < 0 {
    fail "git {have} is too old; install {need} or newer"
}
log.info("git {have} is fine")
```

- `semver.compare(a, b)` gives -1, 0 or 1. It compares the numbers as numbers, so `2.9` is older than `2.30`, and a missing part counts as 0 (`2.30` equals `2.30.0`). A leading `v` is ignored and a pre-release such as `1.0.0-rc.1` is older than `1.0.0`.
- `git --version` prints `git version 2.43.0` on Linux and macOS and `git version 2.43.0.windows.1` on Windows; `regex.find` takes the first three numbers either way. The pattern is written in single quotes, so `\d` reaches the regex as it is.
- A text that isn't a version makes `semver.compare` fail, like any other failure: here the script would stop with `semver: '…' is not a version`.

## Delete old files, after asking

**Goal:** clean up old logs safely: preview with `--dry-run`, confirm interactively, or skip the question with `--yes`.

```nalu
// prune.nalu: deletes .log files that have not changed for a while, after asking.

/// folder to clean, recursively
param dir: path = "logs"
/// delete files older than this
param older_than: duration = 7d
/// do not ask before deleting
param yes: bool = false

var old: []path = []
for file in fs.walk(dir) {
    if file.ext == "log" && time.since(fs.modified(file)) > older_than {
        old.push(file)
    }
}
if old.len() == 0 {
    log.info("nothing older than {older_than} in {dir}")
    exit(0)
}
if !yes && !dry_run && !ui.confirm("Delete {old.len()} files from {dir}?") {
    print("cancelled.")
    exit(1)
}
for file in old {
    fs.rm(file)
}
log.info("deleted {old.len()} files")
```

```console
$ ./prune.macos.sh --dir logs --older-than 30d --dry-run
[dry-run] fs.rm logs/2026-10-01.log
[dry-run] fs.rm logs/2026-10-02.log
info: deleted 2 files
```

- `older_than` becomes `--older-than`; a `bool` param is a flag (`--yes`).
- `ui.confirm` returns false when there is no terminal (in CI or under cron), so unattended runs need `--yes`.
- `dry_run` is true under `--dry-run`. Here it skips the question, since nothing will be deleted; `fs.rm` is only shown, while `log.info` still prints.
- `fs.walk` lists every file under the folder, recursively. `time.since` gives a `duration`, so it compares directly with `older_than`.

## Check a script before you run it

**Goal:** before running something in production, know which lines can stop it and what it would change.

```console
$ nalu check --explain install.nalu
install.nalu:17:11: `fs.temp_dir` can fail → the script stops
install.nalu:21:1: `http.download` can fail → the script stops
install.nalu:24:16: `http.get` can fail → the script stops
install.nalu:24:16: `list[i]` can fail → the script stops
install.nalu:25:4: `hash.sha256_file` can fail → the script stops
install.nalu:26:5: `fail` can fail → the script stops
install.nalu:30:5: `archive.zip_extract` can fail → the script stops
install.nalu:32:5: `archive.tar_extract` can fail → the script stops
install.nalu:36:1: `fs.mkdir` can fail → the script stops
install.nalu:37:1: `fs.copy` can fail → the script stops
install.nalu:38:1: `fs.chmod` can fail → the script stops
11 places can fail without `catch`
$ nalu run install.nalu -- --dry-run      # what it would do, without doing it
```

- `--explain` lists every operation that can fail without a `catch`, and what happens if it does: the function fails, or the script stops. `--json` gives the same for editors and agents.
- `--dry-run` runs reads, `$( )` captures and anything inside a temporary folder, and only shows commands and other writes.
- In CI, `nalu fmt --check .` and `nalu check` on each script catch mistakes before they reach a machine.

More complete scripts are in [`examples/`](../examples/); every function is in the [standard library reference](stdlib.md).
