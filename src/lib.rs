//! Nalu: a compiler from a typed script language to Bash (Linux 4.3+, macOS 3.2) and PowerShell (Windows 5.1+).
//!
//! Pipeline: `lexer → parser (AST) → check (types, scopes, fallibility) → lower (IR per target) → backend`.
//!
//! Frozen contracts (v0.1), written by the orchestrator: [`diag`], [`ir`], [`stdlib`], [`target`].
//! Owners of the other modules: see `docs/history/specs/README.md`.

pub mod diag;
pub mod ir;
pub mod stdlib;
pub mod target;

// Frontend (SPEC-001)
pub mod ast;
pub mod lexer;
pub mod parser;
pub mod check;
pub mod lower;
/// Nested collections (`[][]T`, `map[K][]T`) as flat ones for the backends (D34).
pub mod nest;
/// Portable regular expressions: parsed and checked by the compiler, translated per target (D32).
pub mod regex;

// Tools (D28): formatter and editors
pub mod fmt;
pub mod ide;

// Backends (SPEC-002, SPEC-003)
pub mod bash;
pub mod ps;

pub use target::{Mode, Target};

/// Frontend result: the IR (when there was no error) and all the diagnostics (errors and warnings).
pub struct Compiled {
    pub program: Option<ir::Program>,
    pub diags: Vec<diag::Diag>,
}

/// Runs the whole frontend for one target: source text → IR specialized for `target`.
///
/// - `file`: file name (used in diagnostics and in the header of the generated script).
/// - `embed_root`: base folder for `embed("...")` (the `.nalu` file's folder).
/// - Never panics on user input: errors come back in `diags`.
pub fn compile(source: &str, file: &str, embed_root: &std::path::Path, target: Target) -> Compiled {
    lower::compile(source, file, embed_root, target)
}
