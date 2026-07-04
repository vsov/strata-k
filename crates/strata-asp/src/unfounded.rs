//! Unfounded-set verification. [spec §5.3]
//!
//! Spec §5.3: candidate stability is a *polynomial* unfounded-set check — "which
//! is what motivated the restriction to normal programs". This is the verifier
//! the CPU CDNL core (and any GNN-proposed candidate, §5.4) runs: a valid
//! candidate is a model, an invalid one is the source of a conflict clause.
//!
//! For a normal program and a candidate set `S` (atoms in `S` true, the rest
//! false), `S` is an answer set iff:
//!
//! 1. **`S` is a model** — every rule whose body holds under `S` has its head in
//!    `S`; no constraint's body holds.
//! 2. **`S` has no non-empty unfounded subset** — every atom in `S` has an
//!    *external support*: a rule active under `S` (no negative body atom in `S`)
//!    whose positive body is itself founded. The founded set is the least
//!    fixpoint of that support; the greatest unfounded set is `S \ founded`.
//!
//! This is computed independently of the reduct enumeration in [`crate::solve`],
//! and the two agree on every candidate (see the cross-check test).

use crate::{GRule, Ground};
use std::collections::BTreeSet;

/// The founded (externally supported) subset of `model`: the least set of atoms
/// derivable using only rules not blocked by negation under `model` and whose
/// positive bodies are already founded. Equal to the least model of the
/// Gelfond–Lifschitz reduct restricted to `model`.
pub fn founded_set(model: &BTreeSet<usize>, g: &Ground) -> BTreeSet<usize> {
    // Rules that can support an atom: have a head and are active under `model`
    // (no negative body atom is true in `model`).
    let active: Vec<&GRule> = g
        .rules
        .iter()
        .filter(|r| r.head.is_some() && r.neg.iter().all(|n| !model.contains(n)))
        .collect();

    let mut founded: BTreeSet<usize> = BTreeSet::new();
    loop {
        let mut changed = false;
        for r in &active {
            let h = r.head.unwrap();
            if !founded.contains(&h) && r.pos.iter().all(|p| founded.contains(p)) {
                founded.insert(h);
                changed = true;
            }
        }
        if !changed {
            return founded;
        }
    }
}

/// The greatest unfounded set within `model`: atoms held true with no external
/// support. Empty ⇔ `model` is founded. (Assumes `model` is a classical model;
/// [`is_answer_set`] checks that first.)
pub fn greatest_unfounded_set(model: &BTreeSet<usize>, g: &Ground) -> BTreeSet<usize> {
    let founded = founded_set(model, g);
    model.difference(&founded).copied().collect()
}

/// Is `model` (true atoms; all others false) a classical model of `g` — every
/// applicable rule's head satisfied, no constraint body satisfied?
pub fn is_model(model: &BTreeSet<usize>, g: &Ground) -> bool {
    for r in &g.rules {
        let body_holds =
            r.pos.iter().all(|p| model.contains(p)) && r.neg.iter().all(|n| !model.contains(n));
        if !body_holds {
            continue;
        }
        match r.head {
            Some(h) if !model.contains(&h) => return false, // rule head unsatisfied
            None => return false,                           // constraint fired
            _ => {}
        }
    }
    true
}

/// The spec §5.3 verifier: `model` is an answer set iff it is a classical model
/// and has no non-empty unfounded subset.
pub fn is_answer_set(model: &BTreeSet<usize>, g: &Ground) -> bool {
    is_model(model, g) && greatest_unfounded_set(model, g).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ground, solve, GroundAtom, Val};
    use strata_ir::high::program::{atom, var, Literal, Rule};

    fn a(p: &str) -> strata_ir::high::program::Atom {
        atom(p, vec![])
    }

    /// Map a model of ground atoms to the interned id set used by the verifier.
    fn ids(model: &[GroundAtom], g: &Ground) -> BTreeSet<usize> {
        model
            .iter()
            .map(|m| g.atoms.iter().position(|x| x == m).unwrap())
            .collect()
    }

    /// Every reference answer set passes the unfounded verifier, and *only* the
    /// reference answer sets pass it (brute force over all atom subsets).
    fn cross_check(rules: &[Rule], facts: &[(String, Vec<Val>)], cons: &[Vec<Literal>]) {
        let g = ground(rules, facts, cons).unwrap();
        let reference = solve(rules, facts, cons).unwrap();
        let ref_ids: BTreeSet<BTreeSet<usize>> = reference.iter().map(|m| ids(m, &g)).collect();

        let n = g.n_atoms();
        assert!(n <= 16, "brute force only for small programs");
        let mut verifier_sets: BTreeSet<BTreeSet<usize>> = BTreeSet::new();
        for mask in 0u32..(1u32 << n) {
            let s: BTreeSet<usize> = (0..n).filter(|&i| mask & (1 << i) != 0).collect();
            if is_answer_set(&s, &g) {
                verifier_sets.insert(s);
            }
        }
        assert_eq!(
            ref_ids, verifier_sets,
            "unfounded verifier disagrees with the reduct solver"
        );
    }

    #[test]
    fn unfounded_rejects_self_support() {
        // p :- p.  ⇒ only answer set is {} ({p} is unfounded: p supports itself).
        let g = ground(
            &[Rule {
                head: a("p"),
                body: vec![Literal::Pos(a("p"))],
            }],
            &[],
            &[],
        )
        .unwrap();
        let p = g.atoms.iter().position(|x| x == &("p".to_string(), vec![]));
        if let Some(p) = p {
            let mut s = BTreeSet::new();
            s.insert(p);
            assert!(!is_answer_set(&s, &g), "{{p}} is unfounded");
            assert_eq!(greatest_unfounded_set(&s, &g), s);
        }
        assert!(
            is_answer_set(&BTreeSet::new(), &g),
            "{{}} is the answer set"
        );
    }

    #[test]
    fn cross_check_even_cycle() {
        cross_check(
            &[
                Rule {
                    head: a("a"),
                    body: vec![Literal::Neg(a("b"))],
                },
                Rule {
                    head: a("b"),
                    body: vec![Literal::Neg(a("a"))],
                },
            ],
            &[],
            &[],
        );
    }

    #[test]
    fn cross_check_positive_loop_and_facts() {
        // reach(a). reach(Y) :- edge(a,Y)... modeled simply: q :- p.  p.  ⇒ {p,q}
        cross_check(
            &[Rule {
                head: a("q"),
                body: vec![Literal::Pos(a("p"))],
            }],
            &[("p".to_string(), vec![])],
            &[],
        );
    }

    #[test]
    fn cross_check_choice_with_constraint() {
        // node(a). node(b). in/out per node. :- in(a). (a must be out)
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
        let const_a = strata_ir::high::program::Term::Const { name: "a".into() };
        let cons = vec![vec![Literal::Pos(atom("in", vec![const_a]))]];
        cross_check(&rules, &facts, &cons);
    }
}
