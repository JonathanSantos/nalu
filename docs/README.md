# Nalu documentation

> **Nalu**: *one wave, every shell.* The `nalu` command, `.nalu` files.

A typed script that becomes **Bash** (Linux 4.3+ and macOS 3.2) and **PowerShell** (Windows 5.1+), with nothing to install where it runs.

## Where to start

| Page | What it covers | Read it when |
|---|---|---|
| [Getting started](getting-started.md) | Build the compiler, write and run a first script, handle failures with `catch`, run commands, test | you are new to Nalu |
| [Recipes](recipes.md) | Ready-made scripts: an installer for `curl \| bash` and `irm \| iex`, an API call with a token, JSON config, backups, a CI step | you have a task to do |
| [Language reference](language.md) | Every construct, the failure model, commands, parameters, tests, the CLI and every diagnostic code | you need the exact rule |
| [Standard library](stdlib.md) | Every module, function, method and property, generated from the compiler (`nalu doc --all`) | you need a signature |
| [Vision](vision.md) | The problem, the bets, the non-goals | you want to know why Nalu exists |
| [Architecture](architecture.md) | The pipeline, the IR, the backends, the runtime helpers, the editor tooling, the tests | you work on the compiler |
| [Decisions](decisions.md) | Short records of each design decision and what it replaced | you wonder "why not X?" |

Also: [`llms.txt`](../llms.txt) (the whole language on one page, for a model's context), the [VS Code extension](../editors/vscode/README.md), the [brand](brand/README.md) and the [examples](../examples/README.md). [`history/`](history/README.md) keeps the pre-1.0 design material, in Portuguese.

## Conventions

- Every ` ```nalu ` block in these pages is a complete program that compiles with the current compiler and is already formatted; `cargo test` checks it.
- `stdlib.md` is generated: change `src/stdlib.rs` and run `nalu doc --all > docs/stdlib.md` (a test fails when it is out of date).
- When the reference and the compiler disagree, one of them has a bug; they never stay out of sync.
