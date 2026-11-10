# Nalu for VS Code

Highlighting, live checks, **⚡ on every line that can fail**, **types and docs on hover**, **completion by type**, signature help, go to definition and **formatting** for `.nalu` files, plus GitHub Copilot tools and instructions. The extension has no dependencies and no build step: `extension.js`, `check-json.js` and `copilot.js` call the `nalu` binary, so the compiler answers every question about the language and the editor never guesses.

## Features

| Feature | How it works |
|---|---|
| Highlighting | A TextMate grammar (`syntaxes/nalu.tmLanguage.json`): every keyword, types (`str int bool path duration secret`, `[]T`, `map[K]V`, PascalCase structs and enums), `{expr}` inside `"…"` and `"""…"""` (but not in `'raw'`), `{{`/`}}` and escapes, `$ …` commands, `$(…)` captures, pipes and redirections, durations (`500ms`, `1m30s`, `7d`), numbers (`0xFF`, `1_000`), comments and `///`, standard library modules (`fs.read`, `log.info`…). |
| Live checks | `nalu check --stdin <file> --json --explain` on the editor's text, saved or not: on open, while you type (300 ms after you stop) and on save. Each error or warning has its code (`E010`, `W001`…) and the compiler's `help:`; unused variables and parameters (`W001`, `W002`) are faded. |
| Portability | `W010` warnings (a program that does not exist on a target, such as `grep` on Windows) come from `check`; there is no separate step. |
| Lines that can fail | With `nalu.explain` on, a quiet ⚡ at the end of every line with an operation that can fail without `catch`. The hover says what and what happens: ``⚡ `fs.read` can fail → the script stops``, ``⚡ `$ docker` can fail → `publish` fails``. While the file has compile errors, the marks from the last good check stay. |
| Hover | What any name is: `let names: []str`; `fn publish(s: Service)` with its `///` comment and, if it can fail, why (``It fails when `$ docker` or `$ kubectl` fails``); standard library functions with signature, description, ⚡ when they can fail and ✎ when they write (only shown with `--dry-run`); a `param` with its command-line flag (`--clean-cache`); fields, structs, enums, modules and keywords (`catch`, `defer`, `retry`…). |
| Completion | After `x.`, only what exists for the type of `x` (methods of text, lists and maps, `path` properties, struct fields and methods, `err.message`/`err.code`, enum cases); after `fs.`, the module's functions; at the start of a `match` arm, the enum cases not written yet; after `:` and `->`, types; anywhere else, the names visible at that point (with their types), functions, types, modules and snippets (`fn`, `for`, `match`, `retry`, `test`…). Functions are inserted with their parameters as placeholders. |
| Signature help | On `(` and on every `,`: the signature with the current parameter highlighted, named arguments (`timeout:`, `every:`) included. |
| Go to definition | `F12` or `Cmd`+click on variables, parameters, functions, methods, fields, structs, enums and cases; in `x_test.nalu`, also into `x.nalu`. |
| Formatting | `Format Document` (`Shift`+`Alt`+`F`) runs `nalu fmt --stdin`. Add `"[nalu]": { "editor.formatOnSave": true }` to your settings to format on save. A file with syntax errors is left as it is. |
| Commands | `Nalu: Check the file (nalu check)`, `Nalu: Run in the terminal (nalu run)`, `Nalu: Build the scripts (nalu build)`, `Nalu: Add Copilot instructions to this workspace`. |

Hover, completion, signature help and go to definition call `nalu ide <query> <file> --stdin --at LINE:COLUMN` with the editor's text (see the [CLI reference](../../docs/language.md#the-nalu-tool)); the analysis works on incomplete code too.

## GitHub Copilot

Copilot has never seen Nalu, so on its own it guesses from similar-looking languages. The extension gives it what the compiler knows:

| Where | How |
|---|---|
| Agent mode | Three tools the agent calls on its own (or you, with `#`): `#naluCheck` compiles the code with the real compiler and returns the errors with the suggested fix; `#naluDocs` returns the signature and behavior of any module, function, type or keyword (`nalu doc`); `#naluFormat` formats. The agent writes, checks and fixes before it answers. |
| Chat | The instructions in [`copilot/nalu.instructions.md`](copilot/nalu.instructions.md) (the rules models get wrong most, a sample script, the standard library) ship with the extension, and Copilot applies them when the conversation is about Nalu. `Nalu: Add Copilot instructions to this workspace` copies the file into the project's `.github/instructions/`, for your team and for Copilot on GitHub. |
| Inline suggestions | The extension registers a Copilot context provider that sends the language rules, the names visible at the cursor and the relevant part of the standard library (`nalu ide context`). Today Copilot does not use it: in current VS Code the inline suggestions come from Copilot's next-edit model, and Copilot's public API only feeds the classic completions engine, which also only consults allow-listed providers. The "Nalu" output panel says so when Copilot first asks for the context. Turn it off with `nalu.copilotContext`. |

Instruction files never reach inline suggestions (the VS Code docs say they *"don't apply to inline suggestions as you type"*). What helps inline suggestions today: keep a `.nalu` file with good examples open, since the next-edit model uses recently viewed code, and write a comment saying what the next line should do.

## Requirements

The `nalu` binary. The extension looks for it in this order: the `nalu.path` setting (if you changed it), `target/release/nalu` or `target/debug/nalu` in an open folder (the Nalu repository), and finally `nalu` on your `PATH`:

```bash
cargo build --release
```

If the binary is not found, you get **one** notification (with a button that opens the setting); later attempts fail quietly (only in the "Nalu" output panel) until `nalu.path` changes.

## Install

Without packaging, link this folder into VS Code's extensions folder (from the repository root):

```bash
ln -s "$(pwd)/editors/vscode" ~/.vscode/extensions/nalu-vscode
```

Then reload the window (`Developer: Reload Window`). To build a `.vsix` you can install on other machines:

```bash
npx @vscode/vsce package
```

```bash
code --install-extension nalu-vscode-1.1.0.vsix
```

To work on the extension itself, open `editors/vscode` in VS Code and press F5.

## Settings

| Key | Default | Effect |
|---|---|---|
| `nalu.path` | `nalu` | Path to the binary; accepts `~/…` and paths relative to the open folder (the default also tries `target/release` and `target/debug` in the open folder). |
| `nalu.explain` | `true` | Run `check` with `--explain` and mark the lines that can fail with ⚡. |
| `nalu.lint` | `onType` | `onType` (while typing, on open and on save), `onSave` (on open and on save) or `off`. |
| `nalu.copilotContext` | `true` | Offer Nalu context to GitHub Copilot's inline suggestions (see above). |

## Colors

`catch` and `fail` have scopes of their own, so you can make them stand out in your theme:

| Scope | What |
|---|---|
| `keyword.control.catch.nalu` | `catch` (where a failure is handled) |
| `keyword.control.fail.nalu` | `fail` |
| `meta.command.nalu`, `entity.name.function.command.nalu` | a `$ …` line and the program's name |
| `meta.interpolation.nalu` | a `{expr}` inside text or a command |
| `constant.numeric.duration.nalu` | `500ms`, `1m30s` |

For example, in your user settings:

```json
"editor.tokenColorCustomizations": {
  "textMateRules": [
    { "scope": "keyword.control.catch.nalu", "settings": { "fontStyle": "bold" } },
    { "scope": "keyword.control.fail.nalu", "settings": { "fontStyle": "bold" } }
  ]
}
```
