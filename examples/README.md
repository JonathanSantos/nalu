# Examples

Each example is a complete script that compiles to Bash and PowerShell. More tasks, ready to copy, in [docs/recipes.md](../docs/recipes.md).

| Example | What it does | What it shows |
|---|---|---|
| [hello.nalu](hello.nalu) | Greets you based on the time of day | `param`, `let`, `{x}`, `match` with ranges, `for` with `..=`, chained `catch` |
| [backup.nalu](backup.nalu) | Compresses folders in parallel | `fail`, `catch { }`, `catch fail`, `defer`, `parallel(max:)` with results, `.sum()` |
| [deploy.nalu](deploy.nalu) | Deploys services and waits until they are healthy | an enum and a `secret` as `param`, `if` as an expression, methods with `self`, `match` on an enum, `value \| $ cmd`, `retry`, `parallel for`, `dry_run` |
| [installer.nalu](installer.nalu) | Installs a tool's GitHub release (the `curl \| bash` and `irm \| iex` installer) | `http.get` with headers and `bearer`, `json`, `http.download`, `hash.sha256_file`, `archive`, `fs.chmod`, `os` at compile time |
| [release.nalu](release.nalu) + [tests](release_test.nalu) | Works out the next version, updates the CHANGELOG, creates the tag | `$( )`, a pipe outside Windows only, `..self`, `match` returning structs, `try { } catch fail`, `test`/`assert`/`assert_fails` |

The examples change your system, so `cargo test` only **compiles** them for the three targets and checks the generated syntax (`bash -n`). The `release` tests do run: `nalu test examples`.

```bash
nalu run examples/hello.nalu -- --name Ana --times 2
nalu build examples/deploy.nalu
nalu test examples
```
