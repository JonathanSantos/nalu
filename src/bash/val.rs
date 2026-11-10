#![allow(dead_code)]
//! Helper types of the generator: where a value is ([`Loc`]), what an operation produces ([`Val`]),
//! where it goes ([`Dest`]) and shell conditions ([`Cond`]).

use super::quote::W;
use crate::ir::Ty;

/// Where a composite value (list, map, struct) lives.
#[derive(Clone, Debug)]
pub enum Loc {
    /// Variables from a base name: `xs` (array), `s` (struct: `s__field`), `m` (map: `m__k`…).
    Base(String),
    /// Item `idx` (arithmetic expression) of a list of structs stored in `list` (`list__field[idx]`).
    Elem { list: String, idx: String },
}

/// The result of a call or expression, ready to be copied to a destination.
#[derive(Clone, Debug)]
pub enum Val {
    Unit,
    /// Scalar.
    Word(W),
    /// `bool` as a shell condition (status 0 = `true`).
    Cond(Cond),
    /// List of scalars: the contents of `( … )`, e.g. `"${xs[@]}"` or `'a' 'b'`.
    List(String),
    /// Struct (type name) in `Loc`.
    Struct(Loc, String),
    /// List of structs (prefix, type name): one array per field.
    SList(String, String),
    /// Map: arrays `prefix__k` and `prefix__v` (the index is not used when copying).
    Map(String),
}

/// Where a value goes.
#[derive(Clone, Debug)]
pub enum Dest {
    /// A whole variable (any type), by base name.
    Var(String, Ty),
    /// Scalar list item: array, index (arithmetic).
    ListElem(String, String, Ty),
    /// Item of a list of structs: prefix, index, struct type.
    StructElem(String, String, String),
    /// Map entry: prefix, key, value type.
    MapEntry(String, W, Ty),
    /// Discarded result.
    Discard,
}

/// A shell condition.
#[derive(Clone, Debug, PartialEq)]
pub enum Cond {
    True,
    False,
    /// Inside `(( ))`.
    Arith(String),
    /// Inside `[[ ]]`.
    Test(String),
    /// A command (status 0 = true).
    Cmd(String),
    Not(Box<Cond>),
    And(Vec<Cond>),
    Or(Vec<Cond>),
}

impl Cond {
    pub fn not(c: Cond) -> Cond {
        match c {
            Cond::True => Cond::False,
            Cond::False => Cond::True,
            Cond::Not(x) => *x,
            c => Cond::Not(Box::new(c)),
        }
    }

    fn merge(parts: &[Cond], and: bool) -> Option<Cond> {
        let op = if and { " && " } else { " || " };
        let other = if and { "||" } else { "&&" };
        if parts.iter().all(|p| matches!(p, Cond::Arith(_))) {
            let v: Vec<String> = parts
                .iter()
                .map(|p| match p {
                    Cond::Arith(a) if a.contains(other) || a.contains('?') => format!("({})", a),
                    Cond::Arith(a) => a.clone(),
                    _ => unreachable!(),
                })
                .collect();
            return Some(Cond::Arith(v.join(op)));
        }
        if parts.iter().all(|p| matches!(p, Cond::Test(_))) {
            let v: Vec<String> = parts
                .iter()
                .map(|p| match p {
                    Cond::Test(a) if a.contains(other) => format!("( {} )", a),
                    Cond::Test(a) => a.clone(),
                    _ => unreachable!(),
                })
                .collect();
            return Some(Cond::Test(v.join(op)));
        }
        None
    }

    /// Simplifies and merges adjacent `(( ))`/`[[ ]]`.
    pub fn simplify(self) -> Cond {
        match self {
            Cond::Not(c) => match c.simplify() {
                Cond::True => Cond::False,
                Cond::False => Cond::True,
                Cond::Arith(a) => Cond::Arith(format!("!({})", a)),
                Cond::Test(t) => {
                    if let Some((l, r)) = split_eq(&t, " == ") {
                        Cond::Test(format!("{} != {}", l, r))
                    } else if let Some((l, r)) = split_eq(&t, " != ") {
                        Cond::Test(format!("{} == {}", l, r))
                    } else {
                        Cond::Test(format!("! ( {} )", t))
                    }
                }
                Cond::Not(x) => *x,
                c => Cond::Not(Box::new(c)),
            },
            Cond::And(ps) | Cond::Or(ps) if ps.is_empty() => Cond::True,
            Cond::And(ps) => {
                let mut v = Vec::new();
                for p in ps {
                    match p.simplify() {
                        Cond::True => {}
                        Cond::False => return Cond::False,
                        Cond::And(inner) => v.extend(inner),
                        c => v.push(c),
                    }
                }
                if v.is_empty() {
                    return Cond::True;
                }
                if v.len() == 1 {
                    return v.pop().unwrap();
                }
                Cond::merge(&v, true).unwrap_or(Cond::And(v))
            }
            Cond::Or(ps) => {
                let mut v = Vec::new();
                for p in ps {
                    match p.simplify() {
                        Cond::False => {}
                        Cond::True => return Cond::True,
                        Cond::Or(inner) => v.extend(inner),
                        c => v.push(c),
                    }
                }
                if v.is_empty() {
                    return Cond::False;
                }
                if v.len() == 1 {
                    return v.pop().unwrap();
                }
                Cond::merge(&v, false).unwrap_or(Cond::Or(v))
            }
            c => c,
        }
    }

    /// The condition text as a list of shell commands.
    pub fn render(&self) -> String {
        match self {
            Cond::True => "true".into(),
            Cond::False => "false".into(),
            Cond::Arith(a) => format!("(( {} ))", a),
            Cond::Test(t) => format!("[[ {} ]]", t),
            Cond::Cmd(c) => c.clone(),
            Cond::Not(c) => match **c {
                Cond::And(_) | Cond::Or(_) => format!("! {{ {}; }}", c.render()),
                _ => format!("! {}", c.render()),
            },
            Cond::And(ps) => ps
                .iter()
                .map(|p| match p {
                    Cond::Or(_) => format!("{{ {}; }}", p.render()),
                    _ => p.render(),
                })
                .collect::<Vec<_>>()
                .join(" && "),
            Cond::Or(ps) => ps
                .iter()
                .map(|p| match p {
                    Cond::And(_) => format!("{{ {}; }}", p.render()),
                    _ => p.render(),
                })
                .collect::<Vec<_>>()
                .join(" || "),
        }
    }
}

/// Simple `a == b` (no other logical operators) → (a, b).
fn split_eq<'s>(t: &'s str, op: &str) -> Option<(&'s str, &'s str)> {
    if t.contains("&&") || t.contains("||") || t.matches(op).count() != 1 || t.contains(" != ") && op == " == " || t.contains(" == ") && op == " != " {
        return None;
    }
    let i = t.find(op)?;
    Some((&t[..i], &t[i + op.len()..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn junta_condicoes() {
        let c = Cond::And(vec![Cond::Arith("x > 3".into()), Cond::Arith("y < 2".into())]).simplify();
        assert_eq!(c.render(), "(( x > 3 && y < 2 ))");
        let c = Cond::And(vec![Cond::Arith("x > 3".into()), Cond::Test("$n == 'mundo'".into())]).simplify();
        assert_eq!(c.render(), "(( x > 3 )) && [[ $n == 'mundo' ]]");
        let c = Cond::not(Cond::Test("$a == b".into())).simplify();
        assert_eq!(c.render(), "[[ $a != b ]]");
        let c = Cond::Or(vec![Cond::And(vec![Cond::Cmd("a".into()), Cond::Cmd("b".into())]), Cond::Cmd("c".into())]).simplify();
        assert_eq!(c.render(), "{ a && b; } || c");
    }
}
