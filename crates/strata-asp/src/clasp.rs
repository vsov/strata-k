//! Embedding clasp as the solver. [spec §5.2 "v1 embeds clasp as the solver"]
//!
//! The grounded program ([`crate::Ground`]) is encoded as aspif
//! ([`crate::aspif`]) and handed to a real `clasp` (or clingo's `clasp` core)
//! process; its enumerated models are read back and mapped to ground atoms. This
//! is the production path: our reference [`crate::solve`] is the slow oracle,
//! clasp is the fast CDNL solver whose answer sets must agree with it.
//!
//! `solve_with` returns `None` when the requested binary is not installed, so
//! differential tests skip cleanly (mirroring the Soufflé harness).

use crate::aspif::{parse_show_name, to_aspif};
use crate::{Ground, GroundAtom};
use std::io::Write;
use std::process::{Command, Stdio};

/// Enumerate all answer sets of `g` with an external clasp-compatible solver
/// named `bin` (e.g. `"clasp"`). Feeds aspif on stdin, parses the named models.
///
/// Returns `None` if the binary cannot be spawned (not installed → skip).
/// The returned models are sorted ground-atom lists, matching [`crate::solve`].
pub fn solve_with(bin: &str, g: &Ground) -> Option<Vec<Vec<GroundAtom>>> {
    let doc = to_aspif(g);

    // `0` = enumerate all models. clasp exits 10/20/30 (sat/unsat/complete),
    // which are not errors — parse stdout regardless of the status code.
    let mut child = Command::new(bin)
        .arg("0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?; // binary not installed → skip

    // Write stdin from a thread so a full stdout pipe can't deadlock the write.
    let mut stdin = child.stdin.take()?;
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(doc.as_bytes());
        // drop closes stdin → EOF for the solver.
    });
    let out = child.wait_with_output().ok()?;
    let _ = writer.join();

    let text = String::from_utf8_lossy(&out.stdout);
    Some(parse_models(&text, g))
}

/// clasp's default output: each `Answer: N (…)` line is followed by exactly one
/// line holding that model's shown atoms (blank line = the empty model).
fn parse_models(text: &str, g: &Ground) -> Vec<Vec<GroundAtom>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut models: Vec<Vec<GroundAtom>> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.starts_with("Answer:") {
            continue;
        }
        let model_line = lines.get(i + 1).copied().unwrap_or("");
        let mut atoms: Vec<GroundAtom> = model_line
            .split_whitespace()
            .filter_map(parse_show_name)
            .filter_map(|id| g.atoms.get(id).cloned())
            .collect();
        atoms.sort();
        models.push(atoms);
    }
    models.sort();
    models.dedup();
    models
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ground, solve, Val};
    use strata_ir::high::program::{atom, var, Literal, Rule};

    fn a(p: &str) -> strata_ir::high::program::Atom {
        atom(p, vec![])
    }

    /// clasp (if installed) must agree with the reference solver.
    fn assert_clasp_agrees(rules: &[Rule], facts: &[(String, Vec<Val>)], cons: &[Vec<Literal>]) {
        let g = ground(rules, facts, cons).unwrap();
        let Some(clasp) = solve_with("clasp", &g) else {
            eprintln!("skipping: clasp not installed");
            return;
        };
        let reference = solve(rules, facts, cons).unwrap();
        assert_eq!(reference, clasp, "reference and clasp disagree");
    }

    #[test]
    fn clasp_even_cycle() {
        // a :- not b.  b :- not a.  ⇒ {a}, {b}
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
        assert_clasp_agrees(&rules, &[], &[]);
    }

    #[test]
    fn clasp_constraint_filters() {
        // a :- not b.  b :- not a.  :- a.  ⇒ {b}
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
        let cons = vec![vec![Literal::Pos(a("a"))]];
        assert_clasp_agrees(&rules, &[], &cons);
    }

    #[test]
    fn clasp_first_order_choice() {
        // node(a). node(b). in/out over each ⇒ 4 models
        let facts = vec![
            ("node".to_string(), vec![Val::Sym("a".into())]),
            ("node".to_string(), vec![Val::Sym("b".into())]),
        ];
        let rules = vec![
            Rule {
                head: atom("in", vec![var("X")]),
                body: vec![
                    Literal::Pos(atom("node", vec![var("X")])),
                    Literal::Neg(atom("out", vec![var("X")])),
                ],
            },
            Rule {
                head: atom("out", vec![var("X")]),
                body: vec![
                    Literal::Pos(atom("node", vec![var("X")])),
                    Literal::Neg(atom("in", vec![var("X")])),
                ],
            },
        ];
        assert_clasp_agrees(&rules, &facts, &[]);
    }
}
