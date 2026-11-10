# Decision log

Short records of the choices behind Nalu that someone might want to reopen. Each entry gives the context, the decision and its consequence.

The log is append-only. When a decision changes, its entry stays as written and a later entry supersedes it; the old heading names the new one, as in "(superseded by D27)". Entries without that note are still in force, but they keep the syntax and names of their day:

- Before D30 (v1 syntax), failures were handled with `or` (now `catch`), and types were written `str[]` and `map<str, int>` (now `[]str` and `map[str]int`).
- Before D20, the language was called Onda.
- Compiler and script messages are quoted in English, as they have been since D27.

Dates (YYYY-MM-DD) appear where the original entry had one. For the language as it is today, see [language.md](language.md) and [stdlib.md](stdlib.md); the original Portuguese specifications are in [history/specs/](history/specs/README.md).

## D01 — Name: Onda (superseded by D20)

Shells, literally, are seashells; a wave reaches all of them. Short, pronounceable in Portuguese, English and Spanish, with the CLI name `onda` free and the extension `.onda`. Rejected: Vela (clashes with KubeVela's `vela`), Tide (a fish prompt), Rune (a scripting language for Rust), Ink (Inkle's narrative language).

## D02 — Fail fast by default, `or` to recover

The rule "every fallible call is visibly marked" (Swift, Stela) is great in theory and expensive in scripting practice: 90% of the time the answer is "propagate". With implicit propagation that works the same everywhere, the reader knows what happens without a marker. The cost is that you can't tell from a single line whether it *can* fail; the answer is `nalu check --explain` and the stdlib table. Consequence: there is no `throws` and no `try` prefix; `try` survives only as a block followed by `or`.

## D03 — `or` is right-associative and has the lowest precedence

`env.get("A") or env.get("B") or "x"` must parse as `a or (b or "x")` for chaining to work. `x == 1 or false` reads as "`x == 1`, or `false` if that fails".

## D04 — No `import`

An unused import was an error in Stela; imports are pure ceremony. Stdlib modules are namespaces that are always available. User modules (`use "./lib"`) are left for v0.2, under a different keyword.

## D05 — `{x}` instead of `{{x}}`

Lighter and more common (f-strings, `format!`, C#). In commands, a literal `{}` needs single quotes (`'{}'`), a rare case (`find -exec`). Literal braces in strings: `{{`.

## D06 — `let`/`var`, not `const`/`let`

Two short words; `let` is immutable, as in Rust and Swift. JavaScript users will write `let` expecting it to be mutable, and the compiler explains in one line (`E002`, with a help line suggesting `var`).

## D07 — Linux and macOS get different Bash

Stela levels both down to Bash 3.2. Every relevant Linux has Bash 4.3 or newer; using `declare -A`, `${x,,}` and `wait -n` there gives native maps and less runtime. macOS stays on 3.2. Zsh as a target was considered and rejected: its array and globbing semantics are too different for a third backend.

## D08 — Flat IR (A-normal form) with explicit fallibility

Backends that each walk the AST again drift apart (Stela's QA shows it). Every fallible call becomes an instruction with an `OnFail`; expressions are pure. Cost: more temporaries in PowerShell, which could inline them. Accepted: correctness and equivalence matter more than an elegant `.ps1`.

## D09 — Runtime in `.sh`/`.ps1` files

Helpers kept as escaped Rust strings are unreadable and can't go through shellcheck. `include_str!` plus per-function markers gives the same on-demand inclusion with real files.

## D10 — An unused variable is a warning

An error gets in the way of iterative editing: commenting out a line causes a cascade. `W001` in `check`; CI can use `--deny-warnings` (v0.2).

## D11 — Ordinal text comparison on both targets

Bash compares by locale (3.2 uses `strcmp`, 5 uses `strcoll`); PowerShell compares by culture. The only rule both can reproduce is ordinal, by code point. Consequence: `"a" < "B"` is `false`.

## D12 — Diagnostics in Portuguese, code in English (superseded by D27)

The initial audience is Brazilian and the reference is in Portuguese; keywords and the API are in English so they don't create false friends with the corpus that people and models already know. Messages live in one table (`check.rs`) for future translation.

## D13 — An out-of-range index fails (revised 2026-11-09)

Before: `xs[i]` out of range returned the "empty/zero value" and the script went on. That was a hole in the rule "anything that can fail fails loudly", and inconsistent: in PowerShell, `xs[i] = v` out of range threw; in Bash, it created a sparse array. Now `xs[i]` and `xs[i] = v` with `i < 0` or `i >= xs.len()` fail with `index 9 is out of range (length 2)` and are handled with `or`, like `m[k]` (D14). Lowering puts `if i < 0 || i >= len { fail }` before the access, with the index in a temporary when it is neither a literal nor a variable; the IR and the backends did not change (`Index` is still a pure `Expr`). `&&`, `||` and `while` keep short-circuiting: the check only runs when the access runs. Cost: one arithmetic `if` per access. To grow a list, use `push`.

## D14 — Map index `m[k]` is fallible, `m[k] = v` writes

It reproduces `m.get(k)` with the syntax everyone writes, keeping the "no null" rule: `m[k] or 0`.

## D15 — Corpus questions, decided (2026-10-27)

The corpus agent ([SPEC-004](history/specs/SPEC-004-cli-testes.md)) recorded 19 points where the reference didn't pin down the behavior. The decisions, already applied to the reference:

1. `"""`: the closing line is not included; no trailing `\n`.
2. `"+5".to_int()` fails (digits only, with an optional `-`).
3. Durations as text: fractions only in seconds, up to 3 decimals (`2.5s`, `1m30.5s`); negatives get a leading `-` (`-500ms`).
4. `fs.walk`: includes dotfiles; paths start with `dir` as passed; always `/`.
5. `fs.glob`: sorted; never `.` or `..`.
6. `parallel`: errors in task order.
7. An unhandled error in a `test`: `FAILED  name: <message>`.
8. A failed command: ``command `x` failed with exit code N`` / ``command `x` was not found``.
9. `[dry-run] fs.write file` for stdlib writes.
10. `--verbose` shows substituted values, without quotes.
11. `path` properties expand `~`; `dirs.home()` has no trailing `/`.
12. Strings inside `{ }` in an interpolation: allowed.
13. `let` with the same name in sibling blocks: allowed.
14. `param x: bool = true`: allowed; `--x=false` turns it off.
15. `print(enum)` / `print(bool)`: the case name / `true`.
16. `exit(n)`: `n % 256` on both targets.
17. `<` feeds the first stage of a pipeline; `>`, `2>` and `2>&1` apply to the last.
18. `dirs.cache` / `dirs.data` on Windows: `%LOCALAPPDATA%` / `%APPDATA%`.
19. `nalu test` runs **packages** (`x.nalu` + `x_test.nalu`), not the whole folder.

## D16 — Never leave a large list in `$@` during a loop (Bash 3.2)

Found during integration: Bash 3.2 copies the positional parameters on every nested function call, so `f item1 … item4000` followed by a loop that calls helpers is quadratic (the `mapa` benchmark took 9.4 s). Rule for the Bash backend: functions with a list parameter copy the list into an array **before** the body and clear `$@` (`set --`, or a wrapper that calls the body with no arguments); helpers that receive thousands of items read them from a global array (`__mi_k` in `__map_index`), not from `$@`. Helpers that only do `for x in "$@"` without calling functions can stay as they are.

## D17 — PowerShell runs external programs through `System.Diagnostics.Process`

Instead of `& prog args`. This fixes all of 5.1's traps at once (embedded quotes, empty arguments, `echo`, `2>`, encoding, an extra `\n` on stdin) and gives full control of the command line (MSVCRT quoting rules in `__QArg`). Cost: somewhat more runtime code; `.cmd`/`.bat` files go through `cmd.exe /d /s /c`.

## D18 — `try`/`retry` in Bash are `while :` loops, not functions

`break N`/`continue N` inside the block: cheaper, the user's `return`/`break`/`continue` work directly, and local variables stay visible. The reference described `__try_N` functions; its "How it works" text is a guide, and the observable semantics are the same.

## D19 — Planned modules are a compile error

`regex`, `hash`, `encoding`, `archive` and `pkg` (v0.2) passed `check`, and the script failed only at run time, with code 70. By the rule "what can't work is a compile error", `check` now rejects them with `E090` and suggests `$` or `native`. The backend still generates `__unimplemented` if it receives such a call straight from the IR, as a safety net.

## D20 — Name: Nalu (2026-11-09)

The name moves from Portuguese to Hawaiian and keeps the idea. *Nalu* is the wave that **reaches the shore** (the open-sea wave is *ʻale*): exactly what the language does, one source that reaches the three shells on the beach. The symbol (one wave, three shells) holds without change; the wordmark gained an `l` and a `u` drawn by the same rules as the `n` and the `a` (the `u` is the `n` turned upside down; the `l` is the ascender's stem). Four letters, two syllables, pronounced the same in Portuguese, English and Spanish; no known clash with a language or CLI (Nula, Nala and Nal are close, but they are other projects). Consequence: the `nalu` command, `.nalu` files, `NALU_*` variables (secrets from `NALU_TOKEN`, `NALU_PWSH` in tests) and the `nalu` crate. Scripts and secrets using `ONDA_*` must be renamed.

## D21 — `slice` works as in JavaScript (2026-11-09)

`slice` used `-1` to mean "to the end": `"abcd".slice(0, -1)` gave `"abcd"`, the opposite of JavaScript (`"abc"`) and Python. That is the false friend that D05 and the vision promise to avoid: if the name is borrowed, so are the semantics. Now `end` is optional (to the end), a negative index counts from the end, and out-of-range values are clamped (it never fails), for text and lists alike. The internal default for `end` is `i64::MAX` (`stdlib::SLICE_END`), which the backends clamp to the length. This differs from `xs[i]` (D13), which fails out of range, just as in JavaScript `arr.slice(-1)` works and `arr[-1]` doesn't.

## D22 — `.size` and `.modified` fail like `fs.size` (2026-11-09; superseded by D30)

`p.size` returned 0 for a path that doesn't exist, while `fs.size(p)` failed: two forms of the same thing with opposite semantics, and a silent `0` in a fail-fast language. Now the properties fail with the same message (`p.size or 0` recovers). Along the way, `fs.size` of a folder became 0 on both targets (Bash returned the size of the directory entry).

## D23 — `check --explain` points at lines, not functions (2026-11-09)

D02 bet that tooling would make up for the missing marker (`try`). `--explain` only listed the functions that can fail. Now it lists every point that can fail without `or` (`file:line:column: x can fail → effect`), from the same analysis that infers fallibility, and has a JSON form (`{diagnostics, fallible, fallible_fns}`) for the VS Code extension, which marks those lines in the editor. `check --stdin` reads the source from standard input.

## D24 — `json`, `hash`, `archive`, full `http`, and `fs.chmod` (2026-11-09)

The use case the vision puts first (`curl | bash` and `irm | iex` installers) needs to verify a checksum, extract an archive, read a releases API and mark a binary as executable.

- `json` is parsed by the script itself on all three targets, with the **same algorithm** (a single pass that records the path, type and source text of every value), instead of `ConvertFrom-Json`, which would turn `1.50` into `1.5` and break output equality. In Bash it runs under `LC_ALL=C` (O(1) byte indexing). Reads return the value's source text; `null` fails in `get`.
- `http` gains `headers`, `bearer: secret` and `post`. On Unix, the token reaches `curl` through `-K <(…)`, never on the command line. `wget` is kept only for simple GETs. `download` writes next to the destination and swaps the file in at the end.
- `fs.chmod` on Windows checks that the path exists and does nothing: the honest simulation of "there are no permission bits".
- `regex`, `encoding` and `pkg` stay planned (`E090`).

## D25 — Lists by reference on the linux target (2026-11-09)

Passing a 3,000-item list to a function cost about 6 ms per call in Bash 5 (the items become arguments and are copied). On the `linux` target, a parameter that is a list of scalars is now a `local -n` reference to the array's name, and the same call drops to microseconds (300 calls: 1.8 s → 25 ms). So that a reference never points at the wrong array (Bash has dynamic scope):

1. function-local lists get a program-wide unique name (`__aN_x`), and references are named `__rN_x`;
2. passing a reference on passes its target (`${!__rN_x}`), never the reference itself;
3. top-level lists are passed by name (functions never shadow them: collisions already get a prefix);
4. temporaries and fields are copied into a uniquely named global (`__rvN`), as before;
5. functions that may call themselves, directly or indirectly, still receive a copy.

The semantics are still by value (parameters are immutable). macOS (Bash 3.2, no `local -n`) still copies. The end-to-end tests now also run the `linux` script with a Bash 4.3+ outside Linux (`NALU_BASH4`, or Homebrew's).

## D26 — Brand and design system redone (2026-11-09)

The old symbol (a tilde over three shell fans) was two loose pieces, with details that vanished at small sizes, and the palette (cream sand and coral) and fonts (Outfit, Lexend) fell into the generic look of the moment. The new mark is **a single stroke that reads three ways**: the n (stem and arch), the wave (the crest curling into its own barrel, *nalu*) and the shell (the spiral). Arcs centered on the axis y = 31 with radii 21 → 14 → 8, stroke width 9, round caps; the `n a l u` wordmark got round terminals to match. Color: "lagoon over reef" (Moana; Reef and Lagoon as the signature; Lehua for failure, Limu for success, ʻŌlena for warnings; Sand only in illustrations), with every color pair checked against WCAG at build time. Type: Recursive, one superfamily for prose and code (the Casual axis in headings, Mono for code). The README art (`docs/art/build.py`) uses the same tokens and shapes: ripple lines instead of arrows, generated scripts "on the beach", light and dark, English and Portuguese, all generated together. The main README becomes English, with `README.pt-BR.md` alongside.

## D27 — Messages in English, the same on all three targets (2026-11-09)

Supersedes D12. The main README had become English, and the language targets people who write scripts anywhere. Portuguese messages were the last piece only one audience understood, and generated scripts show them to whoever **runs** a script, not only to whoever writes it. Now everything the compiler says (errors, warnings, `= help:`, `check --explain`, the CLI) and everything a generated script prints (`error:`, `warning:`, `--help`, usage errors, stdlib failures, `nalu test` output, the script header and section markers) is in English. In the translation, Bash and PowerShell messages were unified where they had diverged (`find`, empty list, `sys.cd`, `ui.password`, `Type.parse`, the `fs` errors, `sys.require_admin`, unimplemented features; `proc.which` exits with 127 on both): the text is the same on all three targets, and the end-to-end tests compare the output. `ui.confirm` shows `[y/N]` and accepts `y`/`yes` and `s`/`sim`. What you write (`print`, `fail "…"`, `///`) is unchanged. Still in Portuguese, for now: the reference in `docs/` and the code comments in the runtime helpers (which a generated script copies along with each helper).

## D28 — Formatter and smart editor, both in the compiler (2026-11-09)

**Editor.** Hover with types, type-aware completion, signature help and go-to-definition come from the checker itself, not from a second implementation of the language in the editor. While checking, the checker builds an index (`check::Index`): what each name in the source is (`Sym`: variable, function, stdlib function, method, field, property, struct, enum, case, module, type) and the type of every expression; each variable records the range of source where it is visible. `src/ide.rs` answers queries from that index, and the CLI exposes them as JSON: `nalu ide hover|complete|signature|definition x.nalu --at L:C [--stdin]`. Choices:

- **A CLI instead of an LSP server.** The VS Code extension already called `nalu check --stdin` and stays dependency-free; each query is one process that analyzes the whole file in a few milliseconds (3,500 lines: about 10 ms). A `nalu lsp` command can wrap the same functions when another editor needs it.
- **Analysis of broken code.** Code in an editor is almost always incomplete, so `ide` runs the checker even with syntax errors (the parser recovers). To complete `x.`, the partial name after the dot is replaced with spaces (offsets don't change) and the type comes from the expression that ends before the dot.
- **The documentation is the stdlib's.** The descriptions in the `stdlib.rs` tables (in English, like the messages, D27) appear in hover and completion, with ⚡ when the function can fail and ✎ when it writes (skipped under `--dry-run`). A user function that can fail says why (``It fails when `$ docker` or `$ kubectl` fails``), from the same inference as `--explain`.

**Formatter.** `nalu fmt` preserves line breaks (like `gofmt`, it never joins or splits lines) and normalizes everything else: 4-space indentation per open block, one space around operators and after `,` and `:`, `{ x }` with spaces inside, at most one blank line in a row and none at the start or end of a block. Strings, `$ …` commands and numbers come out exactly as written (inside a command, whitespace is meaningful). The house style aligns columns in two places, and only two: `//` comments at the end of consecutive lines, and the `=>` of `match` arms. With a syntax error, nothing changes. Tests check that formatting twice gives the same text and that the token sequence doesn't change. `fail(`, `retry(` and `parallel(` stay attached to the `(`, and `..`/`..=` attach on both sides (`0..10`, `{ ..self }`), as the repository already wrote them.

The formatter found a lexer bug: the `>` of `map<…>` counted as a comparison at the end of a line, so `field: map<str, int>` followed by another field became a single line (`E005`). Now the lexer tracks `map<…>` (only names, `,`, `[]` and nested `map<…>` inside) and a line break after the `>` that closes the type ends the statement; `a >` at the end of a line still continues on the next line, as before.

## D29 — Copilot learns Nalu from the compiler (2026-11-09)

Code models have never seen Nalu; without help, Copilot completes with Rust and Swift syntax and invents functions. Research done before implementing: inline suggestions use the current file, recently viewed code and recent edits; instruction files (`.github/copilot-instructions.md`, `*.instructions.md`, `AGENTS.md`) apply to chat and agent mode, **not** to inline suggestions. So the extension takes three paths, and in all of them the compiler provides the answers:

- **Inline suggestions:** a Copilot context provider (`getContextProviderAPI('v1')`, registered with `github.copilot` and `github.copilot-chat`, as the C# extension does). On each request, `nalu ide context` returns the rules models most often get wrong (failures and `or`, `$` commands, syntax) as *traits* and, as code snippets, the names visible at the cursor with their types and the part of the stdlib that matters there (modules used in the file, methods of visible types); a short file also gets an example script. It answers in a few milliseconds, caches the last answer per document version and line, and respects the request deadline (`timeoutEnd`). The API has no official documentation and Copilot decides whether to use the context; the statistics it returns (`previousUsageStatistics`) go to the "Nalu" output panel. **Verified in VS Code 1.144 (Copilot's logs and its bundled code):** the provider is registered but never queried. The public API registers providers only for the classic completions engine (`registerContextProvider(…, ["completions"])`), while inline suggestions come from the *next edit* model (`XtabProvider`); and the classic engine only calls allow-listed ids (Copilot's own, the `copilotcontextproviders` experiment variable, or the `github.copilot.advanced.contextProviders` setting). It stays registered anyway (it costs nothing and starts working if Copilot opens the API to *next edit*), with a notice in the panel when the first request arrives.
- **Agent mode:** tools (`languageModelTools`): `nalu_check` compiles with the real compiler and returns the errors with their suggested fixes, `nalu_docs` looks up the documentation (`nalu doc`), and `nalu_format` formats. The agent writes, checks and fixes before it answers.
- **Chat:** `copilot/nalu.instructions.md` ships with the extension (`chatInstructions`); the command `Nalu: Add Copilot instructions to this workspace` copies it to the project's `.github/instructions/`, and the Nalu repository keeps the same copy (a test checks that the two are identical and that their example compiles and is formatted). The instructions are in English, like the messages (D27).

`nalu doc [name…]` was born for the agent tool but serves people too: the page for a module, function, type, member or keyword, built from the stdlib tables; an unknown name gets a suggestion.

## D30 — Nalu 1.0: one way to do each thing (2026-11-09)

Before freezing the language for 1.0, every construct that had a second way to do the same thing lost one, in the spirit of Go: a reader should never wonder which of two spellings is idiomatic, and a model should never have to guess. Each removal is a compile error whose help names the way that stayed, so nothing fails silently.

- **`catch` replaces `or` as the failure handler** (`x catch default`, `x catch fail "…"`, `x catch ignore`, `x catch { … }`, `try { … } catch …`). `or` read like Python's logical *or*, which made `a == 1 or a == 2` the most common beginner mistake (E040 existed only to catch it). `catch` cannot be misread, and it pairs naturally with `try`. `or`, `and` and `not` are now reserved; using them as operators is E006, with a hint that logic is `||`, `&&` and `!`. An unquoted ` catch ` ends a `$` command, which is much rarer as a real argument than `or` was.
- **Types are written like Go: `[]T` and `map[K]V`.** One notation family instead of `T[]` next to `map<K, V>`, and no `<`/`>` in types, which removes the lexer's ambiguity with comparisons (the `map<…>` line-ending bug found by the formatter, D28). The old spellings are E006 with the new one in the help.
- **Removed duplicates** (E001, with the replacement in the help): `m.get(k)`/`m.set(k, v)` (use `m[k]`, which can fail, and `m[k] = v`); `.is_empty()` (use `s == ""`, `xs.len() == 0`); `.to_str()` (interpolation `"{x}"` is the only way to make text); `path.join` (use `/`).
- **Path properties only look at the text.** `.name`, `.stem`, `.ext` and `.parent` never touch the disk and never fail; `.exists`, `.is_dir`, `.is_file`, `.size` and `.modified` are gone in favor of `fs.exists(p)`, `fs.is_dir(p)`, `fs.is_file(p)`, `fs.size(p)` and the new `fs.modified(p)`. A property that could fail (D22) was a surprise; a function call that can fail is not. This supersedes D22.
- **`fail` has one form:** `fail "message"` or `fail "message", code: 3`, like `return`. The parenthesized `fail("message", code: 3)` is E006.
- **Documentation in English.** The reference moved to English to match the README, the messages (D27) and the Copilot instructions; `README.pt-BR.md` stays as the Portuguese entry point and the pre-1.0 design material moved to `docs/history/`. The standard library reference is generated from the compiler (`nalu doc --all`), every ` ```nalu ` block in the docs is compiled by the tests, and the tests check every relative link.

What did not change, on purpose: implicit propagation (no marker on each fallible line; `--explain` and the editor's ⚡ show them), `{x}` as one argument in commands, `match` with exhaustiveness on enums, ranges and `_`, and `retry(…)`/`parallel(…)` taking arguments like a call.

## D31 — 1.0 portability fixes found while writing the docs (2026-11-09)

Writing the English reference meant checking every claim against the three targets, and a few behaviors turned out to differ. Each one now behaves the same everywhere, with an end-to-end case that runs on all three shells:

- **Division by zero fails.** Bash printed its own message, used 0 and went on; PowerShell stopped with .NET's message. Now `a / b` and `a % b` fail with `division by zero`, like an out-of-range index (D13): the checker treats a division whose divisor isn't a non-zero literal as something that can fail (so `--explain` lists it and `catch` handles it), and the lowering puts `if b == 0 { fail "division by zero" }` before it.
- **`~` in commands.** Bash expanded `~/x` even inside quotes, PowerShell never expanded it, and a `path` value starting with `~` reached programs unexpanded on both. Now an unquoted `~` or `~/…` at the start of a command word is the home folder on every target (the lexer turns it into `{dirs.home()}`), a quoted one is text, and a `path` interpolated into a command has its leading `~` expanded, as the standard library already did.
- **`irm … | iex`.** The `.ps1` started with a BOM, which `irm | iex` reads as part of the first command. The PowerShell backend now writes pure ASCII (every other character becomes `$([char]0x…)`), so no BOM is needed and Windows PowerShell 5.1 still reads it correctly; the final `exit` only runs when the script is a file, so a successful `irm | iex` doesn't close the console. The recommended one-liner is `powershell -c "irm URL | iex"`, which isolates every `exit`.
- **`--dry-run` on Windows checks both sides of `fs.move`.** Moving a real folder into a temporary one was carried out (only the destination was checked), and `defer fs.rm(tmp)` would then delete it. Folder moves between drives fall back to copy and delete, like `mv`.

## D32 — `regex` and `encoding`: one regex syntax, checked by the compiler (2026-11-09)

Bash's `[[ =~ ]]` speaks POSIX ERE and PowerShell speaks .NET regex. Passing the user's pattern to each engine would mean two dialects, locale-dependent ranges (`[a-z]` can include capitals under some collations) and different `.`/`$` rules. Instead, a pattern is **fixed text in the call** (`E054` otherwise), which `src/regex.rs` parses into Nalu's own small syntax (literals, `.`, classes with ASCII ranges, `\d \w \s` and their negations, greedy `* + ? {n,m}`, groups, `|`, `^`/`$` at the ends, `(?i)` at the start) and translates for each target, writing out everything the engines would otherwise decide: ranges become explicit character lists in ERE, `\d` becomes `[0-9]` in .NET too, `.` matches a newline everywhere (and a whole surrogate pair in .NET), `$` is the end of the text (`\z`), case-insensitivity becomes `[aA]` classes. The Bash scripts already run in a UTF-8 locale, so `.` is one character.

The two engines still choose differently between several possible matches: POSIX returns the longest at the leftmost start, .NET the first its backtracking finds (`a*(ab)*` on `aab`, `(a|ab)` on `ab`). They agree whenever the pattern can read a text in only one way, which the compiler checks with the Glushkov automaton of the pattern (the classic one-unambiguity test, with character sets instead of symbols), plus three rules for the cases that test doesn't see: no `|` branch that can match empty text, no repetition of something that can match empty text, and no group inside a repeated group. `find`, `find_all`, `groups` and `replace` require such patterns; `regex.test` accepts any valid one, since whether a match exists doesn't depend on the engine. The errors say what to change (`[^=]*` instead of `.*`). `find_all` and `replace` also refuse patterns that can match empty text. `regex.match` was renamed `find` because `match` is a keyword.

`encoding.base64_encode`/`base64_decode` use the system's `base64` in Bash (`-d` on Linux, BusyBox included; `--decode` on macOS) and `[Convert]` in PowerShell, with the same rules on both: spaces and line breaks ignored, the URL-safe alphabet and missing padding accepted, and a failure when the text isn't Base64 or the bytes aren't UTF-8 text (no NUL bytes).

## D33 — `match` with guards, bindings and struct patterns (2026-11-09)

Three additions in Rust's spirit: `pattern if condition =>` (a guard), a name that takes the value (`n => "status {n}"`), and `Type { field: pattern }` for structs (fields left out can be anything; a field can take a literal, a range, a case, `_` or a name). Arms are tried in order and the first one that matches runs. Rules: on an enum, a bare name is always a case, so a typo stays an error (`E001`) instead of silently becoming a binding; a binding is alone in its arm (no `|`); an arm with a guard never counts toward coverage; an arm after one that matches everything is an error (`E031`). Inside a guard, `name =>` at the top level ends the guard instead of starting a lambda.

A plain `match` keeps the backends' `case`; one that uses a guard, a binding or a struct pattern is lowered to a chain of `if`s (a flag lets a failed guard fall through to the next arms), so neither backend changed.

## D34 — Nested collections, encoded before the backends (2026-11-09)

`[][]T` and `map[K][]T` (with `T` a scalar) are allowed. The Bash backend stores collections by naming convention (an array per list, a column per struct field), which has no room for an array inside an array, and teaching both backends nesting would mean two new implementations to keep identical. Instead, an IR pass (`src/nest.rs`) stores each inner list as one text, each item followed by `\x1e`, so the backends only see `[]str` and `map[K]str` plus two internal builtins, `enc_list` and `dec_list`. Whole-collection operations only change type; item reads decode and item writes encode. Equal lists encode to equal texts, so `contains`, `index_of` and `unique` work on whole items. Lists of maps, maps of maps and deeper nesting remain `E090`.

Changing an item in place goes through the checker: `grid[i][j] = v`, `services[0].port = 80` and the statement `groups[k].push(x)` become a copy of the inner value into a hidden variable, the change, and a write back, with the index computed once. Values stay values: `let row = grid[0]` is still a copy.

## D35 — Structs inside structs, flattened (2026-11-09)

A struct field can be another struct (`port: Port`). The same IR pass flattens it into the outer struct (`port__number`, `port__proto`), so `d.svc.port.number` is a plain field access on both backends, lists of such structs keep the column layout, and a nested struct used as a whole value is rebuilt from its fields. The checker rejects a struct that contains itself, directly or through other structs, and flattened names that would clash with an existing field. A list of structs as a field is still `E090`.

## D36 — `parallel` runs at the same time on Windows (2026-11-09)

PowerShell now runs `parallel` tasks in a `RunspacePool` (available in Windows PowerShell 5.1), with the semantics of the Bash background jobs: every task runs, errors are joined in task order with the first failed task's code, each task gets its own copy of the script's variables, and an `exit` in a task ends only that task. The folder and the environment belong to the whole process, so tasks that call `sys.cd` or `env.set` (found through the call graph) run one at a time, and the parent's folder and environment are restored afterwards. When a pool can't be created (Constrained Language Mode), the tasks run one after another, as in 1.0.

The same work fixed the Bash side: a `parallel for` that returns structs gave an empty list (each task now sends a record that is split back into the list's columns), `max` was ignored after the first `parallel` on Bash 5.1+ (where `wait -n` can return at once; the slot logic now counts live tasks and falls back to waiting for the oldest), and `max: 0` looped forever on macOS (below 1 now counts as 1).

Smaller changes in 1.1: a program used only inside `if proc.exists("x") { … }` is optional (not required at startup, no `W010`); a list of `path` values passed to a program has its leading `~` expanded like a single one, on every target (`~user` stays as written); `param` accepts `[]path`; the `||` hint in commands points to `$ cmd catch { $ other }`, which now works on one line.

## D37 — `use`: one script from several files (2026-11-09)

Scripts grow, and copying helper functions between them was the only way to share them. `use "lib.nalu"` at the start of a file brings in another file's declarations. Choices:

- **The CLI joins the files; the language stays one file.** The same machinery that already compiled `x_test.nalu` with `x.nalu` (one text with an offset map that sends each diagnostic back to its file) now also appends every used file, found recursively relative to the file that uses it, each once. The output is still one self-contained script per target, which keeps the promise that nothing has to be installed or copied next to it. The editor queries build the same combined text, so hover, completion and go to definition cross files.
- **A used file only declares** `fn`, `struct`, `enum` and `test` (`E055` otherwise). A library with top-level code or `param`s would run that code in every script that uses it, which is never what a `use` means.
- **One namespace.** No module prefixes (`text.shout`): scripts are small, the standard library already owns the dotted names, and a clash is a plain `E003`.
- **`use` is not a keyword**: it is only special as `use "…"` at the start of a file, so existing scripts with a variable called `use` keep compiling.

## D38 — The last gaps in the 1.1 syntax (2026-11-09)

- **Lists of structs as fields** (`services: []Service`). The flattening of D35 now also turns such a field into one list per field of the item (`services__name`, `services__port`, and so on, recursively), so the backends still see only scalars and lists. An item is rebuilt from those lists when used as a whole (`cfg.services[0]`), a whole list when it is read (`for s in cfg.services`) and split again when it is written; `cfg.services[i].name` reads `services__name[i]` directly. Items whose fields are maps are still `E090`, and a struct that contains a list of itself (a tree) is `E010`.
- **Mutating methods on nested values as values.** `let x = rows[0].pop()`, `x = groups[k].pop()` and `return rows[0].pop()` use the same copy and write back as `rows[0].push(v)` (D34); for `return`, the copy is written back before returning. Inside a larger expression it stays `E002`, because the write back needs a statement of its own.
- **`code`, `max` and `every` are ordinary names.** They were reserved because they are the labels of named arguments (`fail "…", code: 3`, `retry(3, every: 1s)`, `parallel(max: 4)`), but the parser recognizes those by `name:`, never by looking up a variable.

## D39 — A variable for one program: `NAME=value` before it (2026-11-10)

Build and deploy scripts set variables for a single program all the time (`CGO_ENABLED=0 go build`, `NODE_ENV=production node …`). Nalu had only `env.set`, which changes the whole script, and the shell spelling compiled without complaint and then failed at run time looking for a program named `CGO_ENABLED=0`: a false friend of the kind the language promises not to have. Choices:

- **The shell's own spelling**, because that is what everyone types. A word is an assignment when it comes before the program and starts with `NAME=`, where `NAME` is an identifier; the checker takes these words off the front of each stage, so nothing else in the pipeline changed (the program is still `words[0]`, for the dependency check and the messages). After the program, `a=b` stays an argument.
- **The IR carries it** (`SimpleCmd.env`), and each backend uses its native form. Bash writes the assignment in front of the program itself, which is why a command with variables is not sent through the `__cmd` helper: there the assignment would also be visible inside the helper. PowerShell fills `ProcessStartInfo.EnvironmentVariables`.
- **Secrets are welcome there.** A program's environment, unlike its arguments, does not show up in the process list, and neither backend puts the value on a command line. So `TOKEN={token}` gives no `W011`, and it is the way to hand a token to a program that reads one from its environment.

Found on the way: PowerShell named only the program in ``command `x` failed with exit code N`` where Bash and the reference name the whole command; and its `--verbose` text fell back to the bare words when the command had `{` inside single quotes or a redirection to a fixed file. Both now match Bash (the two backends share the scan for `{…}`).

## D40 — 1.2: the last planned module and the gaps real scripts hit (2026-11-10)

The additions came from writing the scripts the vision puts first and noting every place where the answer was "write it by hand" or "it fails at run time":

- **`pkg`** was the only module still refused as planned. `pkg.manager()` returns the first manager found, in a fixed order per system; `pkg.install` runs it without questions, with `sudo` on Linux when the script is not root, and sends the manager's output to standard error, because standard output belongs to the script. Package names are not translated between managers: no table would stay right, and `pkg.manager()` lets the script choose. On `apt-get`, a failed install is retried once after `apt-get update`, since a fresh container has no package lists. A real install runs in CI on Ubuntu, Alpine and macOS.
- **A program the script asks about with `proc.exists` is not required up front.** Without this, `if !proc.exists("jq") { pkg.install(["jq"]) }` followed by `$ jq …` stopped before its first line with `this script needs: jq`. Asking is the script saying it will deal with the answer. `W010` is unchanged: it still looks only at the branch the question guards.
- **`semver.compare`** because two of the documented recipes compared versions by hand, each with its own bug waiting (text order puts `1.10` before `1.9`). It follows the precedence rules of Semantic Versioning, relaxed where real tools are: any number of numeric parts, an optional `v`. Both targets run the same algorithm and reject the same texts.
- **`err` after `catch value`.** `catch fail "…{err}"` already had `err` outside a block; the plain value did not, so `catch err.code` needed three lines. When the value uses `err`, the checker handles it as the one-line block it stands for.
- **`http.request`** covers `PUT`, `PATCH` and `DELETE` with one function instead of three. `GET` stays `http.get`: it has no body and nothing to show under `--dry-run`.
- **`archive.zip_create`** writes entries with `/` on Windows itself, after CI on Windows PowerShell 5.1 showed .NET's `CreateFromDirectory` writing `\` there.
- **`xs.concat(ys)`**, a method and not `+`: `+` on lists would be the only operator that allocates a collection, and the method works on struct and nested lists through the same column and encoding schemes as `slice`.

Considered and left out: floats (no shell has them; sizes and percentages work as integers), `.map()` returning structs, lists of maps, and a folder for one command or block. The last one is the most missed; a block form (`sys.cd(dir) { … }`) needs `return`, `break` and `continue` to restore the folder on the way out, which deserves its own design.
