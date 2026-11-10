//! Standard library registry. FROZEN CONTRACT (v0.1): the **source of truth** for the API.
//!
//! - The frontend uses these tables to resolve names, check types and know what is fallible.
//! - Each backend must implement **all** the entries marked `v1: true` (a test in each backend
//!   checks this). The `v1: false` ones are planned: the checker rejects the call with `E090` (D19); the backend
//!   still generates `__unimplemented` if it receives one directly from the IR.
//! - Types as text, in Nalu's syntax: `str`, `int`, `bool`, `path`, `duration`, `secret`, `unit`,
//!   `[]str`, `map[str]int`. Method generics: `T` (item), `U` (result of `map`), `K`, `V`.
//!   `any` = any scalar that becomes text (`print`, `log`). `...` = variadic (zero or more `any`).
//!   `Enum` = the receiver's own enum (`Type.parse`).
//! - `writes: true` = an effect on the world: under `--dry-run` the operation is **shown** and not executed
//!   (except inside a folder/file created by `fs.temp_dir`/`fs.temp_file` in this run).

#[derive(Clone, Copy, Debug)]
pub struct StdParam {
    pub name: &'static str,
    pub ty: &'static str,
    /// Nalu literal for the default (`"30s"`, `"\"\""`, `"-1"`), or `None` if required.
    pub default: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
pub struct StdFn {
    pub module: &'static str,
    pub name: &'static str,
    pub params: &'static [StdParam],
    pub ret: &'static str,
    pub fallible: bool,
    pub writes: bool,
    pub v1: bool,
    pub doc: &'static str,
}

/// Default of `slice`'s `end`: "to the end" (the backend clamps it to the length). It is not `-1`, which counts from the end.
pub const SLICE_END: &str = "9223372036854775807";

const fn p(name: &'static str, ty: &'static str) -> StdParam {
    StdParam { name, ty, default: None }
}
const fn pd(name: &'static str, ty: &'static str, default: &'static str) -> StdParam {
    StdParam { name, ty, default: Some(default) }
}
const fn f(module: &'static str, name: &'static str, params: &'static [StdParam], ret: &'static str, fallible: bool, writes: bool, v1: bool, doc: &'static str) -> StdFn {
    StdFn { module, name, params, ret, fallible, writes, v1, doc }
}

/// Functions `module.name(...)`. No `import` is needed: the modules are always available.
pub static STDLIB: &[StdFn] = &[
    // env
    f("env", "get", &[p("name", "str")], "str", true, false, true, "The value of the environment variable; fails if it is not set."),
    f("env", "has", &[p("name", "str")], "bool", false, false, true, "Whether the environment variable is set."),
    f("env", "set", &[p("name", "str"), p("value", "str")], "unit", false, false, true, "Sets the variable for this script and for the commands it runs."),
    // fs
    f("fs", "exists", &[p("p", "path")], "bool", false, false, true, "Whether the path exists (file or folder)."),
    f("fs", "is_dir", &[p("p", "path")], "bool", false, false, true, "Whether the path is an existing folder."),
    f("fs", "is_file", &[p("p", "path")], "bool", false, false, true, "Whether the path is an existing file."),
    f("fs", "read", &[p("p", "path")], "str", true, false, true, "Reads the whole file as UTF-8 text (a BOM is removed)."),
    f("fs", "write", &[p("p", "path"), p("text", "str")], "unit", true, true, true, "Creates or overwrites the file: UTF-8 without BOM, `\\n` line endings."),
    f("fs", "append", &[p("p", "path"), p("text", "str")], "unit", true, true, true, "Adds text to the end of the file, creating it if needed."),
    f("fs", "rm", &[p("p", "path")], "unit", true, true, true, "Deletes a file or folder, recursively; no error if it does not exist. Refuses \"\", /, ., .., ~ and the current folder."),
    f("fs", "mkdir", &[p("p", "path")], "unit", true, true, true, "Creates the folder and its parents (like `mkdir -p`); no error if it already exists."),
    f("fs", "copy", &[p("from", "path"), p("to", "path")], "unit", true, true, true, "Copies a file, or a folder recursively."),
    f("fs", "move", &[p("from", "path"), p("to", "path")], "unit", true, true, true, "Moves or renames a file or folder."),
    f("fs", "ls", &[p("dir", "path")], "[]str", true, false, true, "The names of the entries in the folder (without . and ..), sorted by bytes."),
    f("fs", "walk", &[p("dir", "path")], "[]path", true, false, true, "Every file under the folder, recursively, as paths that start with `dir` as given, sorted."),
    f("fs", "glob", &[p("pattern", "str")], "[]path", false, false, true, "The paths that match the pattern: `*` and `?` only in the last component; entries starting with `.` only if the pattern does; empty if nothing matches."),
    f("fs", "temp_dir", &[], "path", true, false, true, "Creates a new temporary folder. Writes inside it run even with --dry-run."),
    f("fs", "temp_file", &[], "path", true, false, true, "Creates a new temporary file. Writes to it run even with --dry-run."),
    f("fs", "size", &[p("p", "path")], "int", true, false, true, "The size in bytes; fails if the path does not exist."),
    f("fs", "modified", &[p("p", "path")], "int", true, false, true, "The last modification time, in Unix seconds; fails if the path does not exist."),
    f("fs", "chmod", &[p("p", "path"), p("mode", "str")], "unit", true, true, true, "Unix: `chmod` (\"755\", \"+x\", \"u+x\"). Windows: checks the path and does nothing (there are no permission bits)."),
    f("fs", "replace", &[p("p", "path"), p("from", "str"), p("to", "str")], "unit", true, true, true, "Replaces every occurrence of literal text inside the file."),
    // path (functions; the properties `.name` etc. are in PATH_PROPS)
    f("path", "abs", &[p("p", "path")], "path", false, false, true, "The absolute path, with . and .. resolved."),
    // dirs
    f("dirs", "home", &[], "path", false, false, true, "The user's home folder."),
    f("dirs", "temp", &[], "path", false, false, true, "The system's temporary folder."),
    f("dirs", "config", &[p("app", "str")], "path", false, false, true, "The app's config folder: $XDG_CONFIG_HOME/app or ~/.config/app; ~/Library/Application Support/app; %APPDATA%\\app."),
    f("dirs", "cache", &[p("app", "str")], "path", false, false, true, "The app's cache folder on this system."),
    f("dirs", "data", &[p("app", "str")], "path", false, false, true, "The app's data folder on this system."),
    f("dirs", "cwd", &[], "path", false, false, true, "The current folder."),
    // sys
    f("sys", "arch", &[], "str", false, false, true, "The CPU architecture: x64, arm64…"),
    f("sys", "hostname", &[], "str", false, false, true, "The machine's name."),
    f("sys", "user", &[], "str", false, false, true, "The current user's name."),
    f("sys", "is_admin", &[], "bool", false, false, true, "Whether the script runs as root / Administrator."),
    f("sys", "require_admin", &[], "unit", true, false, true, "Fails with exit code 77 unless the script runs as root / Administrator."),
    f("sys", "cpus", &[], "int", false, false, true, "The number of logical CPUs."),
    f("sys", "is_ci", &[], "bool", false, false, true, "Whether the CI environment variable is set."),
    f("sys", "cd", &[p("dir", "path")], "unit", true, false, true, "Changes the script's current folder, until the end of the function or script."),
    // proc
    f("proc", "exists", &[p("cmd", "str")], "bool", false, false, true, "Whether the program is on PATH."),
    f("proc", "which", &[p("cmd", "str")], "path", true, false, true, "The full path of the program; fails (exit code 127) if it is not on PATH."),
    // time
    f("time", "now", &[], "int", false, false, true, "The current time, in Unix seconds."),
    f("time", "now_ms", &[], "int", false, false, true, "The current time, in Unix milliseconds."),
    f("time", "since", &[p("t", "int")], "duration", false, false, true, "The time elapsed since an instant from `time.now()` (second precision)."),
    f("time", "since_ms", &[p("t", "int")], "duration", false, false, true, "The time elapsed since an instant from `time.now_ms()`."),
    f("time", "format", &[pd("fmt", "str", "\"%Y-%m-%d %H:%M:%S\""), pd("t", "int", "-1")], "str", false, false, true, "Formats an instant (`t = -1` is now). Only %Y %y %m %d %H %M %S %j %s %%."),
    f("time", "sleep", &[p("d", "duration")], "unit", false, false, true, "Waits for the given duration."),
    f("time", "hour", &[pd("t", "int", "-1")], "int", false, false, true, "The local hour, 0 to 23 (`t = -1` is now)."),
    // log (stderr, with a prefix; debug only with --verbose)
    f("log", "info", &[p("items", "...")], "unit", false, false, true, "Prints `info: …` to stderr."),
    f("log", "warn", &[p("items", "...")], "unit", false, false, true, "Prints `warning: …` to stderr."),
    f("log", "error", &[p("items", "...")], "unit", false, false, true, "Prints `error: …` to stderr. It does not stop the script; `fail` does."),
    f("log", "debug", &[p("items", "...")], "unit", false, false, true, "Prints `debug: …` to stderr, only with --verbose."),
    // ui
    f("ui", "confirm", &[p("question", "str")], "bool", false, false, true, "Asks a yes/no question (`[y/N]`); false when there is no terminal."),
    f("ui", "ask", &[p("question", "str"), pd("fallback", "str", "\"\"")], "str", false, false, true, "Asks for a line of text; returns `fallback` when there is no terminal."),
    f("ui", "password", &[p("question", "str")], "secret", true, false, true, "Asks for a secret, without echoing what is typed."),
    f("ui", "choose", &[p("question", "str"), p("options", "[]str")], "str", true, false, true, "Shows a numbered menu and returns the chosen option."),
    // math
    f("math", "min", &[p("a", "int"), p("b", "int")], "int", false, false, true, "The smaller of two integers."),
    f("math", "max", &[p("a", "int"), p("b", "int")], "int", false, false, true, "The larger of two integers."),
    f("math", "abs", &[p("n", "int")], "int", false, false, true, "The absolute value."),
    // http (Unix: curl; wget only for GET without headers; Windows: Invoke-WebRequest). `headers`: "Name: value";
    // `bearer`: becomes `Authorization: Bearer …` and never appears in process arguments, logs or --verbose.
    f("http", "get", &[p("url", "str"), pd("timeout", "duration", "30s"), pd("headers", "[]str", "[]"), pd("bearer", "secret", "\"\"")], "str", true, false, true, "Sends a GET and returns the body; fails on network errors and on status >= 400."),
    f("http", "status", &[p("url", "str"), pd("timeout", "duration", "30s"), pd("headers", "[]str", "[]"), pd("bearer", "secret", "\"\"")], "int", true, false, true, "Sends a GET and returns only the HTTP status code."),
    f("http", "download", &[p("url", "str"), p("to", "path"), pd("headers", "[]str", "[]"), pd("bearer", "secret", "\"\"")], "unit", true, true, true, "Downloads the URL into a file; fails on status >= 400 without leaving a partial file."),
    f("http", "post", &[p("url", "str"), p("body", "str"), pd("content_type", "str", "\"application/json\""), pd("timeout", "duration", "30s"), pd("headers", "[]str", "[]"), pd("bearer", "secret", "\"\"")], "str", true, true, true, "Sends a POST and returns the response body; fails on status >= 400. With --dry-run it is only shown (and returns \"\")."),
    // json (no dependencies: interpreted by the script itself on all three targets). Path: "a.b.0" or "a.b[0]"; "" is the root.
    f("json", "get", &[p("text", "str"), p("path", "str")], "str", true, false, true, "The value at the path: text without quotes; numbers, bools, lists and objects as their JSON text. Fails if it is missing, null, or the text is not valid JSON."),
    f("json", "get_list", &[p("text", "str"), p("path", "str")], "[]str", true, false, true, "The items of the list at the path, each as in `get` (null becomes \"null\")."),
    f("json", "keys", &[p("text", "str"), p("path", "str")], "[]str", true, false, true, "The keys of the object at the path, in the order of the text."),
    f("json", "has", &[p("text", "str"), p("path", "str")], "bool", false, false, true, "Whether the path exists and is not null (invalid JSON: false)."),
    f("json", "quote", &[p("text", "str")], "str", false, false, true, "The text as a JSON string (quotes and escapes), for building JSON."),
    // regex (D32): the pattern is fixed text in Nalu's syntax, checked and translated by the compiler
    f("regex", "test", &[p("text", "str"), p("pattern", "str")], "bool", false, false, true, "Whether the pattern matches somewhere in the text. The pattern is fixed text in Nalu's regex syntax, checked by the compiler; write it in single quotes: `regex.test(v, '^\\d+$')`."),
    f("regex", "find", &[p("text", "str"), p("pattern", "str")], "str", true, false, true, "The first match; fails if there is none. The pattern must have a single way to match (the compiler checks it)."),
    f("regex", "find_all", &[p("text", "str"), p("pattern", "str")], "[]str", false, false, true, "Every match, left to right; none gives an empty list. The pattern must have a single way to match and can't match empty text."),
    f("regex", "groups", &[p("text", "str"), p("pattern", "str")], "[]str", true, false, true, "The groups `( … )` of the first match, in order (a group that took no part is \"\"); fails if there is no match."),
    f("regex", "replace", &[p("text", "str"), p("pattern", "str"), p("new", "str")], "str", false, false, true, "Replaces every match with `new`, taken literally (no `$1`). The pattern must have a single way to match and can't match empty text."),
    // hash / encoding
    f("hash", "sha256", &[p("text", "str")], "str", false, false, true, "The SHA-256 of the text's UTF-8 bytes, in lowercase hex."),
    f("hash", "sha256_file", &[p("p", "path")], "str", true, false, true, "The SHA-256 of the file, in lowercase hex; fails if it does not exist."),
    f("encoding", "base64_encode", &[p("text", "str")], "str", false, false, true, "The Base64 of the text's UTF-8 bytes: standard alphabet, `=` padding, one line."),
    f("encoding", "base64_decode", &[p("text", "str")], "str", true, false, true, "Decodes Base64 into text. Spaces and line breaks are ignored, the URL-safe alphabet (`-` `_`) and missing `=` padding are accepted; fails if the text isn't Base64 or the bytes aren't UTF-8 text."),
    // archive
    f("archive", "tar_create", &[p("file", "path"), p("dir", "path")], "unit", true, true, true, "Creates a .tar.gz with the folder inside (the folder's name is the archive's root)."),
    f("archive", "tar_extract", &[p("file", "path"), p("to", "path")], "unit", true, true, true, "Extracts a .tar.gz into `to`, creating it."),
    f("archive", "zip_extract", &[p("file", "path"), p("to", "path")], "unit", true, true, true, "Extracts a .zip into `to`, creating it. Unix: unzip (or BSD tar); Windows: Expand-Archive."),
    // pkg
    f("pkg", "manager", &[], "str", false, false, false, "The system's package manager: apt-get, dnf, yum, pacman, zypper, apk; brew; winget, choco, scoop; or \"\". Planned; the compiler refuses it for now (E090)."),
    f("pkg", "install", &[p("names", "[]str")], "unit", true, true, false, "Installs packages with the system's manager, with sudo on Linux when not root. Planned; the compiler refuses it for now (E090)."),
];

#[derive(Clone, Copy, Debug)]
pub struct Method {
    /// `str` (also applies to `path`), `list`, `map`, `enum`, `duration`, `int`, `bool`.
    pub recv: &'static str,
    pub name: &'static str,
    pub params: &'static [StdParam],
    pub ret: &'static str,
    pub fallible: bool,
    /// Mutates the receiver (only on a `var`): `push`, `set`, `remove`, `pop`.
    pub mutates: bool,
    pub doc: &'static str,
}

const fn m(recv: &'static str, name: &'static str, params: &'static [StdParam], ret: &'static str, fallible: bool, mutates: bool, doc: &'static str) -> Method {
    Method { recv, name, params, ret, fallible, mutates, doc }
}

/// Methods of the built-in types. `len()` is not here: it is `ExprKind::Len`. Names in snake_case, Python/Rust
/// semantics: `replace` replaces **all** occurrences, `split` keeps empty fields, indexes start at 0,
/// `slice(a, b)` excludes `b`, as in JS: without `b` it goes to the end; a negative counts from the end (`slice(0, -1)` drops the last);
/// out of range is clamped, never fails (D21). Text is compared and sorted **ordinally**.
pub static METHODS: &[Method] = &[
    // str / path
    m("str", "trim", &[], "str", false, false, "Removes whitespace from both ends."),
    m("str", "trim_start", &[], "str", false, false, "Removes whitespace from the start."),
    m("str", "trim_end", &[], "str", false, false, "Removes whitespace from the end."),
    m("str", "upper", &[], "str", false, false, "An uppercase copy."),
    m("str", "lower", &[], "str", false, false, "A lowercase copy."),
    m("str", "contains", &[p("s", "str")], "bool", false, false, "Whether `s` occurs in the text."),
    m("str", "starts_with", &[p("s", "str")], "bool", false, false, "Whether the text starts with `s`."),
    m("str", "ends_with", &[p("s", "str")], "bool", false, false, "Whether the text ends with `s`."),
    m("str", "strip_prefix", &[p("s", "str")], "str", false, false, "Removes `s` from the start, if it is there."),
    m("str", "strip_suffix", &[p("s", "str")], "str", false, false, "Removes `s` from the end, if it is there."),
    m("str", "index_of", &[p("s", "str")], "int", false, false, "The character index of the first `s`, or -1."),
    m("str", "slice", &[p("start", "int"), pd("end", "int", SLICE_END)], "str", false, false, "The characters from `start` up to, not including, `end`. Negative counts from the end; out of range is clamped."),
    m("str", "split", &[p("sep", "str")], "[]str", false, false, "Splits on `sep`, keeping empty fields."),
    m("str", "lines", &[], "[]str", false, false, "Splits into lines (\\n or \\r\\n), without the trailing empty line."),
    m("str", "replace", &[p("from", "str"), p("to", "str")], "str", false, false, "Replaces every occurrence of literal text."),
    m("str", "replace_first", &[p("from", "str"), p("to", "str")], "str", false, false, "Replaces the first occurrence of literal text."),
    m("str", "repeat", &[p("n", "int")], "str", false, false, "The text repeated `n` times."),
    m("str", "pad_left", &[p("n", "int"), pd("fill", "str", "\" \"")], "str", false, false, "Pads on the left with `fill` up to `n` characters."),
    m("str", "pad_right", &[p("n", "int"), pd("fill", "str", "\" \"")], "str", false, false, "Pads on the right with `fill` up to `n` characters."),
    m("str", "to_int", &[], "int", true, false, "Parses a 64-bit integer, ignoring surrounding spaces; fails otherwise."),
    m("str", "chars", &[], "[]str", false, false, "One item per character."),
    // []T
    m("list", "push", &[p("x", "T")], "unit", false, true, "Adds the item to the end (only on a `var`)."),
    m("list", "pop", &[], "T", true, true, "Removes and returns the last item (only on a `var`); fails if the list is empty."),
    m("list", "filter", &[p("f", "(T) -> bool")], "[]T", false, false, "The items for which `f` returns true."),
    m("list", "map", &[p("f", "(T) -> U")], "[]U", false, false, "A new list with `f` applied to each item (`f` returns a scalar)."),
    m("list", "any", &[p("f", "(T) -> bool")], "bool", false, false, "Whether `f` returns true for some item."),
    m("list", "all", &[p("f", "(T) -> bool")], "bool", false, false, "Whether `f` returns true for every item."),
    m("list", "find", &[p("f", "(T) -> bool")], "T", true, false, "The first item for which `f` returns true; fails if there is none."),
    m("list", "contains", &[p("x", "T")], "bool", false, false, "Whether the list has the item."),
    m("list", "index_of", &[p("x", "T")], "int", false, false, "The index of the first equal item, or -1."),
    m("list", "join", &[p("sep", "str")], "str", false, false, "Joins the items with `sep` (lists of scalars only)."),
    m("list", "sum", &[], "T", false, false, "The sum of a `[]int` or `[]duration`."),
    m("list", "sort", &[], "[]T", false, false, "Sorted ascending: text in ordinal order, numbers by value."),
    m("list", "sort_by", &[p("f", "(T) -> U")], "[]T", false, false, "A stable sort by the key `f` returns (`int` or `str`)."),
    m("list", "reverse", &[], "[]T", false, false, "The items in reverse order."),
    m("list", "slice", &[p("start", "int"), pd("end", "int", SLICE_END)], "[]T", false, false, "The items from `start` up to, not including, `end`. Negative counts from the end; out of range is clamped."),
    m("list", "unique", &[], "[]T", false, false, "Without duplicates, keeping the first occurrence."),
    m("list", "first", &[], "T", true, false, "The first item; fails if the list is empty."),
    m("list", "last", &[], "T", true, false, "The last item; fails if the list is empty."),
    // map[K]V
    m("map", "has", &[p("k", "K")], "bool", false, false, "Whether the key exists."),
    m("map", "remove", &[p("k", "K")], "unit", false, true, "Removes the key (only on a `var`); no error if it does not exist."),
    m("map", "keys", &[], "[]K", false, false, "The keys, in insertion order."),
    m("map", "values", &[], "[]V", false, false, "The values, in insertion order."),
    // enum
    m("enum", "parse", &[p("text", "str")], "Enum", true, false, "`Type.parse(\"prod\")`: the case with that name; fails if there is none."),
];

/// Properties of `path` (without parentheses): they only look at the path's text, never at the disk, and never fail.
pub static PATH_PROPS: &[(&str, &str, &str)] = &[
    ("name", "str", "The last component, with its extension."),
    ("stem", "str", "The last component, without its extension."),
    ("ext", "str", "The extension without the dot; \"\" if there is none (dotfiles have none)."),
    ("parent", "path", "The containing folder; the root's parent is the root."),
];

/// Removed in v1 (D30, "one way to do each thing"): `(receiver or module, name, the way that remains)`.
/// The receiver is the method category (`str`, `list`, `map`…), `path` for properties and `module:` for
/// module functions. The checker uses the help in the error message.
pub static REMOVED: &[(&str, &str, &str)] = &[
    ("map", "get", "read with `m[k]`; it can fail: `m[k] catch default`"),
    ("map", "set", "write `m[k] = v`"),
    ("str", "is_empty", "compare with an empty string: `s == \"\"`"),
    ("list", "is_empty", "check the length: `xs.len() == 0`"),
    ("map", "is_empty", "check the length: `m.len() == 0`"),
    ("int", "to_str", "use interpolation: `\"{x}\"`"),
    ("bool", "to_str", "use interpolation: `\"{x}\"`"),
    ("duration", "to_str", "use interpolation: `\"{d}\"`"),
    ("enum", "to_str", "use interpolation: `\"{x}\"`"),
    ("path", "exists", "properties of `path` never touch the disk; use `fs.exists(p)`"),
    ("path", "is_dir", "properties of `path` never touch the disk; use `fs.is_dir(p)`"),
    ("path", "is_file", "properties of `path` never touch the disk; use `fs.is_file(p)`"),
    ("path", "size", "properties of `path` never touch the disk; use `fs.size(p)`"),
    ("path", "modified", "properties of `path` never touch the disk; use `fs.modified(p)`"),
    ("module:path", "join", "join paths with `/`: `base / child`"),
];

/// The help for something removed in v1, if `name` was removed from `recv` (see [`REMOVED`]).
pub fn removed(recv: &str, name: &str) -> Option<&'static str> {
    REMOVED.iter().find(|(r, n, _)| *r == recv && *n == name).map(|(_, _, h)| *h)
}

/// Builtins without a module. `fail`, `exit`, `retry`, `parallel`, `defer`, `assert`, `assert_fails`, `embed` and
/// `secret` are handled by the grammar/checker, but `secret`, `print`, `fmt` reach the backend as `Builtin`.
pub static BUILTINS: &[StdFn] = &[
    f("", "print", &[p("items", "...")], "unit", false, false, true, "Writes the items to standard output, separated by spaces, with a newline at the end."),
    f("", "fmt", &[p("template", "str"), p("items", "...")], "str", false, false, true, "Replaces each `{}` in the template with the next item (only `{}`, no format specifiers)."),
    f("", "secret", &[p("text", "str")], "secret", false, false, true, "Turns text into a secret: never printed, never passed in a command's arguments."),
    f("", "duration_str", &[p("d", "duration")], "str", false, false, true, "Internal: a duration as text (1m30s)."),
    f("", "enum_parse", &[p("text", "str")], "Enum", true, false, true, "Internal: `Type.parse(text)`."),
    f("", "enc_list", &[p("items", "[]str")], "str", false, false, true, "Internal: a list as one text, each item followed by \\x1e (D34)."),
    f("", "dec_list", &[p("text", "str")], "[]str", false, false, true, "Internal: the list `enc_list` wrote; `ret` says the item type (D34)."),
];

/// Builtins the compiler inserts itself; they never show up in completion or docs.
pub fn internal_builtin(name: &str) -> bool {
    matches!(name, "duration_str" | "enum_parse" | "enc_list" | "dec_list")
}

/// Built-in constants: `os` (resolved at compile time), `dry_run: bool`, `args: []str`, `self` (methods), `err` (handlers).
pub static RESERVED: &[&str] = &["os", "dry_run", "args", "self", "err", "print", "fmt", "secret", "embed", "fail", "exit", "retry", "assert", "assert_fails", "or", "and", "not"];

/// One line about each module (hover and completion in the editor).
pub static MODULES: &[(&str, &str)] = &[
    ("archive", "Creates and extracts .tar.gz and .zip archives."),
    ("dirs", "Home, temporary, config, cache and data folders on each system."),
    ("encoding", "Base64 encoding and decoding of text."),
    ("env", "Environment variables."),
    ("fs", "Files and folders. Writes are only shown with --dry-run."),
    ("hash", "SHA-256 of text and files."),
    ("http", "HTTP requests, with headers and a `bearer` token that never shows up in a command's arguments."),
    ("json", "Reads JSON with paths like \"a.b[0]\", parsed by the script itself (no jq)."),
    ("log", "Leveled messages on stderr."),
    ("math", "Integer helpers."),
    ("path", "Path helpers. Properties like `.name` and `.parent` are read from `path` values."),
    ("pkg", "System package managers. Planned; the compiler refuses it for now (E090)."),
    ("proc", "Programs on PATH."),
    ("regex", "Regular expressions in one portable syntax, checked by the compiler and identical on every target."),
    ("sys", "Architecture, user, admin and CI checks, the current folder."),
    ("time", "Timestamps, durations, formatting and waiting."),
    ("ui", "Questions for the user that fall back to a default without a terminal."),
];

pub fn module_doc(name: &str) -> Option<&'static str> {
    MODULES.iter().find(|(n, _)| *n == name).map(|(_, d)| *d)
}

/// Keywords, constants and builtins without a signature table: `(name, summary, documentation)`.
pub static KEYWORDS: &[(&str, &str, &str)] = &[
    ("let", "let name = value", "Declares a name that cannot change. Use `var` for one that can."),
    ("var", "var name = value", "Declares a variable that can change (`=`, `+=`, `.push(…)`)."),
    ("fn", "fn name(param: type) -> type { … }", "Declares a function. It can fail if something inside it can fail without `catch`; the compiler works that out."),
    ("return", "return value", "Ends the function, returning the value."),
    ("struct", "struct Name { field: type }", "Declares a record type with fields and methods (`fn` inside it, with `self`)."),
    ("enum", "enum Name { a, b, c }", "Declares a type with a fixed set of cases: `Name.a`, `Name.parse(\"a\")`."),
    ("param", "param name: type = default", "A command-line option of the script: `--name value`. The `///` comment above it becomes its `--help` text; without a default it is required."),
    ("if", "if cond { … } else { … }", "Runs a block when the condition is true. As a value, both branches give the value and `else` is required."),
    ("else", "else { … }", "The branch of an `if` that runs when the condition is false."),
    ("for", "for x in list { … }", "Repeats the block for each item of a list, each `key, value` of a map, or each number of a range (`0..10`)."),
    ("in", "for x in list", "Separates the loop names from what is being iterated."),
    ("while", "while cond { … }", "Repeats the block while the condition is true."),
    ("match", "match value { pattern => result }", "Runs the first arm that matches the value: text, numbers, ranges, durations, bools, enum cases or `Struct { field: pattern }`. `_` matches anything, a name (`n => …`) takes the value, and `pattern if condition =>` adds a guard."),
    ("break", "break", "Leaves the innermost loop."),
    ("continue", "continue", "Skips to the next iteration of the innermost loop."),
    ("fail", "fail \"message\"", "Fails with a message (and optionally `code: n`). The function fails, or the script stops, unless a caller handles it with `catch`."),
    ("catch", "risky() catch fallback", "Handles a failure on its left: `catch value`, `catch fail \"…\"`, `catch ignore` or `catch { … }` (with `err`). Without `catch`, failures propagate: the function fails, or the script stops."),
    ("ignore", "catch ignore", "Handles a failure by doing nothing."),
    ("try", "try { … } catch …", "Groups several lines under one `catch`: the first failure jumps to the handler."),
    ("defer", "defer call()", "Runs the call when the function or script ends, even after a failure or Ctrl+C."),
    ("parallel", "parallel for x in list { … }", "Runs the tasks at the same time and waits for all of them; if any fail, it fails with every error. `parallel(max: n)` limits how many run at once."),
    ("retry", "retry(3, every: 5s) { … }", "Runs the block again until it does not fail, up to n times; then fails with the last error."),
    ("native", "native { bash: \"…\", powershell: \"…\" }", "Raw shell code for each target, when the standard library has no portable way."),
    ("test", "test \"name\" { … }", "A test, run by `nalu test`. Use `assert cond` and `assert_fails(call)` inside it."),
    ("use", "use \"lib.nalu\"", "Brings in the `fn`, `struct`, `enum` and `test` declarations of another file. `use` lines go at the start of the file and the path is relative to it; the script still compiles to one self-contained file per target."),
    ("self", "self", "The value a method was called on (inside `fn` declared in a `struct`). Read-only."),
    ("true", "true", "The `bool` value true."),
    ("false", "false", "The `bool` value false."),
    ("os", "os: str", "The target system: \"linux\", \"macos\" or \"windows\". A compile-time constant, so `if os == …` keeps only the matching branch in each script."),
    ("dry_run", "dry_run: bool", "Whether the script runs with --dry-run (commands and writes are only shown)."),
    ("args", "args: []str", "The command-line arguments that are not options (and everything after `--`)."),
    ("err", "err: error", "The failure being handled, inside `catch { … }` and `catch fail \"…{err}\"`: `err.message` and `err.code`."),
    ("exit", "exit(code)", "Ends the script now with the exit code (runs `defer` first)."),
    ("embed", "embed(\"file.txt\") -> str", "The contents of a text file, read at compile time (the path is relative to the .nalu file)."),
    ("assert", "assert cond", "In a test: fails the test when the condition is false, showing the condition."),
    ("assert_fails", "assert_fails(call)", "In a test: fails the test when the call does not fail."),
    ("_", "_ => …", "In a `match`, matches any value the arms above did not."),
];

pub fn keyword_doc(name: &str) -> Option<(&'static str, &'static str)> {
    KEYWORDS.iter().find(|(n, _, _)| *n == name).map(|(_, s, d)| (*s, *d))
}

pub fn find_fn(module: &str, name: &str) -> Option<&'static StdFn> {
    STDLIB.iter().find(|f| f.module == module && f.name == name)
}

pub fn find_method(recv: &str, name: &str) -> Option<&'static Method> {
    METHODS.iter().find(|m| m.recv == recv && m.name == name)
}

pub fn find_builtin(name: &str) -> Option<&'static StdFn> {
    BUILTINS.iter().find(|f| f.name == name)
}

/// All module names, for suggestions ("did you mean `fs`?").
pub fn modules() -> Vec<&'static str> {
    let mut v: Vec<&str> = STDLIB.iter().map(|f| f.module).collect();
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_duplicates() {
        for (i, a) in STDLIB.iter().enumerate() {
            for b in &STDLIB[i + 1..] {
                assert!(!(a.module == b.module && a.name == b.name), "duplicate: {}.{}", a.module, a.name);
            }
        }
        for (i, a) in METHODS.iter().enumerate() {
            for b in &METHODS[i + 1..] {
                assert!(!(a.recv == b.recv && a.name == b.name), "duplicate: {}.{}", a.recv, a.name);
            }
        }
    }

    #[test]
    fn documentation_is_complete_and_single_line() {
        // shown in hover, completion and the Copilot context: every entry has a description, without a line break
        let docs = STDLIB.iter().map(|f| f.doc).chain(METHODS.iter().map(|m| m.doc)).chain(PATH_PROPS.iter().map(|p| p.2)).chain(MODULES.iter().map(|m| m.1));
        for d in docs {
            assert!(!d.is_empty() && !d.contains('\n'), "{:?}", d);
        }
        for m in modules() {
            assert!(module_doc(m).is_some(), "module without a description: {}", m);
        }
    }

    #[test]
    fn lookup() {
        assert!(find_fn("fs", "read").unwrap().fallible);
        assert!(!find_fn("fs", "exists").unwrap().fallible);
        assert!(find_method("list", "push").unwrap().mutates);
        assert_eq!(modules().first(), Some(&"archive"));
    }
}
