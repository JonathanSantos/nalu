//! Bash backend (SPEC-002): IR → Bash script for Linux (Bash 4.3+) and macOS (Bash 3.2).
//!
//! Layout:
//! - [`gen`]: generator state, frames (top level/functions), script assembly, statements and control flow;
//! - [`expr`]: pure expressions (words, arithmetic, conditions, where collections live);
//! - [`store`]: copy by value into destinations (`let`, assignment, return, literals, maps);
//! - [`call`]: calls (user, builtins, stdlib, methods with lambdas) and `parallel`;
//! - [`cmd`]: external commands (`$`, `$( )`, `if $`), `--verbose`/`--dry-run`;
//! - [`params`]: command-line reader;
//! - [`quote`]: all quoting (`sq`, `dq_esc`, `W`);
//! - [`runtime`]: `runtime/bash/*.sh` helpers, on demand.
//!
//! Execution model (summary; details in `docs/architecture.md`):
//! no `set -e`; each fallible operation is followed by the context's failure action (`__fail` at top level,
//! `return 1` in a function, `break N`/`continue N` inside `try`/`retry`, which become `while :` loops);
//! return values in `__ret`/`__ret_a`/`__ret__field`/`__ret_m__k|v`; structs flattened (`s__field`);
//! lists of structs with one array per field; maps in `m__k`/`m__v` + index (`declare -A` on Linux,
//! one variable per key on macOS).

mod call;
mod cmd;
mod expr;
mod gen;
mod params;
mod quote;
pub mod runtime;
mod store;
mod val;

#[cfg(all(test, unix))]
mod tests;

use crate::ir::Program;
use crate::target::Mode;

/// Generates the Bash script for `program.target` (`Linux` or `Macos`).
pub fn generate(program: &Program, mode: Mode) -> String {
    gen::Gen::new(program, mode).run()
}
