//! On-demand PowerShell runtime: the helpers live in `runtime/ps/*.ps1`, one block per helper, marked with
//! `# @fn Name` and (optionally) `# @needs A B`. The generated script includes only the blocks it uses (transitive
//! closure), in the file order below and, within each file, in order of appearance.

/// The runtime files, in inclusion order.
pub const FILES: &[(&str, &str)] = &[
    ("core.ps1", include_str!("../../runtime/ps/core.ps1")),
    ("text.ps1", include_str!("../../runtime/ps/text.ps1")),
    ("list.ps1", include_str!("../../runtime/ps/list.ps1")),
    ("path.ps1", include_str!("../../runtime/ps/path.ps1")),
    ("fs.ps1", include_str!("../../runtime/ps/fs.ps1")),
    ("sys.ps1", include_str!("../../runtime/ps/sys.ps1")),
    ("data.ps1", include_str!("../../runtime/ps/data.ps1")),
    ("cmd.ps1", include_str!("../../runtime/ps/cmd.ps1")),
    ("params.ps1", include_str!("../../runtime/ps/params.ps1")),
];

#[derive(Debug, Clone)]
pub struct Helper {
    pub name: String,
    pub needs: Vec<String>,
    pub text: String,
    pub file: &'static str,
}

/// All `# @fn` blocks, in inclusion order.
pub fn helpers() -> Vec<Helper> {
    let mut out: Vec<Helper> = Vec::new();
    for (file, src) in FILES {
        let mut cur: Option<Helper> = None;
        for line in src.lines() {
            let t = line.trim();
            if let Some(name) = t.strip_prefix("# @fn ") {
                if let Some(h) = cur.take() {
                    out.push(h);
                }
                cur = Some(Helper { name: name.trim().to_string(), needs: vec![], text: String::new(), file });
                continue;
            }
            if let Some(h) = cur.as_mut() {
                if let Some(n) = t.strip_prefix("# @needs ") {
                    h.needs.extend(n.split_whitespace().map(String::from));
                    continue;
                }
                h.text.push_str(line.trim_end());
                h.text.push('\n');
            }
        }
        if let Some(h) = cur.take() {
            out.push(h);
        }
    }
    for h in &mut out {
        while h.text.ends_with("\n\n") {
            h.text.pop();
        }
    }
    out
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Does `name` appear in `code` as a whole word (not as a prefix of another name)?
pub fn mentions(code: &str, name: &str) -> bool {
    let b = code.as_bytes();
    let mut from = 0;
    while let Some(i) = code[from..].find(name) {
        let s = from + i;
        let e = s + name.len();
        let before_ok = s == 0 || !is_ident(b[s - 1]);
        let after_ok = e >= b.len() || !is_ident(b[e]);
        if before_ok && after_ok {
            return true;
        }
        from = s + 1;
    }
    false
}

/// The runtime text that `code` uses, with its dependencies.
pub fn select(code: &str) -> String {
    let all = helpers();
    let mut used: Vec<bool> = all.iter().map(|h| mentions(code, &h.name)).collect();
    loop {
        let mut changed = false;
        for i in 0..all.len() {
            if !used[i] {
                continue;
            }
            for n in &all[i].needs {
                if let Some(j) = all.iter().position(|h| &h.name == n) {
                    if !used[j] {
                        used[j] = true;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut out = String::new();
    for (i, h) in all.iter().enumerate() {
        if used[i] {
            out.push_str(&h.text);
            out.push('\n');
        }
    }
    out
}
