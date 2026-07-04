//! aspif emission — the clasp-compatible intermediate format. [spec §5.2]
//!
//! Spec §5.2: the grounder streams surviving rules to the CPU "in a binary
//! aspif-compatible format (compatibility with clasp is essential: v1 embeds
//! clasp as the solver)". This module is the textual aspif encoder for a
//! grounded normal program ([`crate::Ground`]); [`crate::clasp`] pipes it to a
//! real clasp/clingo process and reads back the models.
//!
//! aspif atoms are 1-based positive integers, so interned id `k` becomes atom
//! `k + 1`. A positive body literal is `+atom`, a negative one is `-atom`. Every
//! atom is emitted with an output (`show`) statement named `s<id>` so the solver
//! echoes a readable model we can map back to ground atoms.

use crate::Ground;
use std::fmt::Write as _;

/// The show-symbol for interned atom `id` (round-trips via [`parse_show_name`]).
pub fn show_name(id: usize) -> String {
    format!("s{id}")
}

/// Inverse of [`show_name`]: `"s7"` → `Some(7)`.
pub fn parse_show_name(s: &str) -> Option<usize> {
    s.strip_prefix('s')?.parse().ok()
}

/// aspif literal for a positive/negative interned atom id (1-based, signed).
fn lit(id: usize, positive: bool) -> i64 {
    let a = id as i64 + 1;
    if positive {
        a
    } else {
        -a
    }
}

/// Encode a grounded normal program as an aspif text document.
///
/// - a rule with a head → `1 0 1 <head> 0 <#body> <body…>` (disjunctive head of
///   one atom = a normal rule);
/// - a constraint (no head) → `1 0 0 0 <#body> <body…>` (empty disjunctive head);
/// - every atom → `4 <len> s<id> 1 <atom>` so models come back named.
pub fn to_aspif(g: &Ground) -> String {
    let mut s = String::from("asp 1 0 0\n");

    for r in &g.rules {
        let body: Vec<i64> = r
            .pos
            .iter()
            .map(|&p| lit(p, true))
            .chain(r.neg.iter().map(|&n| lit(n, false)))
            .collect();
        match r.head {
            Some(h) => {
                // rule, head_type 0 (disjunctive), 1 head atom, then normal body.
                let _ = write!(s, "1 0 1 {} 0 {}", h + 1, body.len());
            }
            None => {
                // constraint: disjunctive head of zero atoms.
                let _ = write!(s, "1 0 0 0 {}", body.len());
            }
        }
        for l in body {
            let _ = write!(s, " {l}");
        }
        s.push('\n');
    }

    // Output statements: one per atom, condition = that atom's own literal.
    for id in 0..g.n_atoms() {
        let name = show_name(id);
        let _ = writeln!(s, "4 {} {} 1 {}", name.len(), name, id + 1);
    }

    s.push_str("0\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ground, Val};
    use strata_ir::high::program::{atom, var, Literal, Rule};

    fn a(p: &str) -> strata_ir::high::program::Atom {
        atom(p, vec![])
    }

    #[test]
    fn even_cycle_aspif_shape() {
        // a :- not b.  b :- not a.
        let rules = vec![
            Rule {
                head: a("a"),
                body: vec![Literal::Neg(a("b"))],
            },
            Rule {
                head: a("b"),
                body: vec![Literal::Neg(a("a"))],
            },
        ];
        let g = ground(&rules, &[], &[]).unwrap();
        let doc = to_aspif(&g);
        assert!(doc.starts_with("asp 1 0 0\n"));
        assert!(doc.trim_end().ends_with("0")); // end statement
                                                // two rules + two show statements + header + end.
        assert_eq!(doc.lines().filter(|l| l.starts_with("1 ")).count(), 2);
        assert_eq!(doc.lines().filter(|l| l.starts_with("4 ")).count(), 2);
    }

    #[test]
    fn show_name_roundtrip() {
        for id in [0usize, 1, 42, 1000] {
            assert_eq!(parse_show_name(&show_name(id)), Some(id));
        }
        assert_eq!(parse_show_name("x3"), None);
    }

    #[test]
    fn constraint_has_empty_head() {
        // fact a.  :- a.
        let facts = vec![("a".to_string(), vec![] as Vec<Val>)];
        let constraints = vec![vec![Literal::Pos(a("a"))]];
        let g = ground(&[], &facts, &constraints).unwrap();
        let doc = to_aspif(&g);
        // the constraint line: "1 0 0 0 1 <lit>"
        assert!(
            doc.lines().any(|l| l.starts_with("1 0 0 0 1 ")),
            "missing constraint line in:\n{doc}"
        );
    }

    #[test]
    fn var_rule_grounds_and_emits() {
        // node(a). node(b). in(X) :- node(X), not out(X).
        let facts = vec![
            ("node".to_string(), vec![Val::Sym("a".into())]),
            ("node".to_string(), vec![Val::Sym("b".into())]),
        ];
        let rules = vec![Rule {
            head: atom("in", vec![var("X")]),
            body: vec![
                Literal::Pos(atom("node", vec![var("X")])),
                Literal::Neg(atom("out", vec![var("X")])),
            ],
        }];
        let g = ground(&rules, &facts, &[]).unwrap();
        let doc = to_aspif(&g);
        // one show statement per distinct ground atom.
        assert_eq!(
            doc.lines().filter(|l| l.starts_with("4 ")).count(),
            g.n_atoms()
        );
    }
}
