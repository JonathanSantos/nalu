//! Runtime helpers in `runtime/bash/*.sh`, included with `include_str!`.
//!
//! Each file is a sequence of blocks:
//!
//! ```text
//! # @fn __name
//! # @needs __other __more
//! # @target linux|macos|all
//! __name() { … }
//! ```
//!
//! The generated script gets only the helpers it uses (transitive closure of `@needs`), in the order they
//! appear in the files (deterministic).

use std::collections::{BTreeSet, HashMap};

pub const FILES: &[(&str, &str)] = &[
    ("core.sh", include_str!("../../runtime/bash/core.sh")),
    ("str.sh", include_str!("../../runtime/bash/str.sh")),
    ("list.sh", include_str!("../../runtime/bash/list.sh")),
    ("map.sh", include_str!("../../runtime/bash/map.sh")),
    ("fs.sh", include_str!("../../runtime/bash/fs.sh")),
    ("sys.sh", include_str!("../../runtime/bash/sys.sh")),
    ("data.sh", include_str!("../../runtime/bash/data.sh")),
];

#[derive(Clone, Debug)]
pub struct Helper {
    pub name: String,
    pub needs: Vec<String>,
    /// `None` = all targets; `Some(true)` = linux only; `Some(false)` = macos only.
    pub linux_only: Option<bool>,
    pub body: String,
}

/// All blocks of all files, in order.
pub fn parse_all() -> Vec<Helper> {
    let mut out = Vec::new();
    for (_, src) in FILES {
        let mut cur: Option<Helper> = None;
        for line in src.lines() {
            if let Some(name) = line.strip_prefix("# @fn ") {
                if let Some(h) = cur.take() {
                    out.push(finish(h));
                }
                cur = Some(Helper { name: name.trim().to_string(), needs: vec![], linux_only: None, body: String::new() });
                continue;
            }
            let Some(h) = cur.as_mut() else { continue };
            if let Some(n) = line.strip_prefix("# @needs") {
                h.needs.extend(n.split_whitespace().map(|s| s.to_string()));
            } else if let Some(t) = line.strip_prefix("# @target") {
                h.linux_only = match t.trim() {
                    "linux" => Some(true),
                    "macos" => Some(false),
                    _ => None,
                };
            } else {
                h.body.push_str(line);
                h.body.push('\n');
            }
        }
        if let Some(h) = cur.take() {
            out.push(finish(h));
        }
    }
    out
}

fn finish(mut h: Helper) -> Helper {
    while h.body.ends_with("\n\n") {
        h.body.pop();
    }
    h
}

/// The helpers of this target, by name.
pub fn for_target(linux: bool) -> Vec<Helper> {
    parse_all().into_iter().filter(|h| h.linux_only.map_or(true, |l| l == linux)).collect()
}

/// The text of the requested helpers and their dependencies, in file order.
pub fn emit(wanted: &BTreeSet<String>, linux: bool) -> String {
    let all = for_target(linux);
    let by_name: HashMap<&str, &Helper> = all.iter().map(|h| (h.name.as_str(), h)).collect();
    let mut need: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = wanted.iter().cloned().collect();
    while let Some(n) = stack.pop() {
        if !need.insert(n.clone()) {
            continue;
        }
        match by_name.get(n.as_str()) {
            Some(h) => stack.extend(h.needs.iter().cloned()),
            None => panic!("unknown runtime helper: {}", n),
        }
    }
    let mut o = String::new();
    for h in &all {
        if need.contains(&h.name) {
            o.push_str(&h.body);
        }
    }
    o
}

/// Is there a helper with this name for the target?
pub fn exists(name: &str, linux: bool) -> bool {
    for_target(linux).iter().any(|h| h.name == name)
}
