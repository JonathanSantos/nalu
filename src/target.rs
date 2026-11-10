//! Compilation targets and generation modes. FROZEN CONTRACT (v0.1).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// Bash 4.3+ (`#!/usr/bin/env bash`). May use `declare -A`, `${x,,}`, `mapfile`, `wait -n`.
    Linux,
    /// Bash 3.2 (macOS's `/bin/bash`). No associative arrays, no `${x^^}`, no `mapfile`, no `wait -n`.
    Macos,
    /// Windows PowerShell 5.1+ (`.ps1` in UTF-8 with CRLF, and a BOM only if it has non-ASCII bytes) + `.cmd` launcher.
    Windows,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::Linux, Target::Macos, Target::Windows];

    /// Accepts `linux`, `macos` (and `darwin`, `mac`), `windows` (and `win`).
    pub fn parse(s: &str) -> Option<Target> {
        match s.to_ascii_lowercase().as_str() {
            "linux" => Some(Target::Linux),
            "macos" | "darwin" | "mac" => Some(Target::Macos),
            "windows" | "win" => Some(Target::Windows),
            _ => None,
        }
    }

    /// The value of the `os` constant inside the script: `"linux"`, `"macos"` or `"windows"`.
    pub fn os_name(self) -> &'static str {
        match self {
            Target::Linux => "linux",
            Target::Macos => "macos",
            Target::Windows => "windows",
        }
    }

    /// Suffix of the generated file: `x.linux.sh`, `x.macos.sh`, `x.windows.ps1`.
    pub fn file_suffix(self) -> &'static str {
        match self {
            Target::Linux => "linux.sh",
            Target::Macos => "macos.sh",
            Target::Windows => "windows.ps1",
        }
    }

    pub fn is_bash(self) -> bool {
        !matches!(self, Target::Windows)
    }

    /// The target of the system the compiler is running on.
    pub fn current() -> Target {
        if cfg!(target_os = "windows") {
            Target::Windows
        } else if cfg!(target_os = "macos") {
            Target::Macos
        } else {
            Target::Linux
        }
    }
}

/// What the backend must generate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The complete script: parameters, dependencies, declarations and the top-level body.
    Script,
    /// Only pure declarations + `test` blocks; the top-level body does not run. Output: one line per test and a summary.
    Tests,
}
