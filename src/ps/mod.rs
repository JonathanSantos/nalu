//! PowerShell backend (SPEC-003): IR → script for **Windows PowerShell 5.1** (also runs on 7) and the
//! `.cmd` launcher.
//!
//! - [`generate`] returns the text with `\n`; the CLI writes the `.ps1` with a UTF-8 BOM and CRLF.
//! - The runtime comes from `runtime/ps/*.ps1` (see [`runtime`]): only the helpers in use go into the script.
//! - [`check_syntax`] is a minimal 5.1 syntax checker for when the machine has no PowerShell.

use crate::ir::Program;
use crate::target::Mode;

mod gen;
pub mod runtime;
mod syntax;

pub use gen::{lit, mangle};
pub use syntax::check_syntax;

pub fn generate(program: &Program, mode: Mode) -> String {
    gen::Gen::new(program).program(mode)
}

pub fn launcher(script_name: &str) -> String {
    format!(
        "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0{}.windows.ps1\" %*\r\nexit /b %ERRORLEVEL%\r\n",
        script_name
    )
}

#[cfg(test)]
mod tests;
