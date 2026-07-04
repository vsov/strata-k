//! Intelligent grounding: simplify a ground program before it reaches the
//! solver. [spec §5.2]
//!
//! Spec §5.2 step 2: "simplification on the GPU before transfer: removing rules
//! with definitely-false bodies, fact substitution, a pass of deterministic unit
//! propagation, removal of duplicate and subsumed rules. This is the functional
//! equivalent of gringo's intelligent grounding, without which CDNL would get a
//! bloated input." This module is the CPU reference for that pass; the
//! device-parallel version lives in [`strata-gpu`](../../strata-gpu) and is
//! checked bit-for-bit against it.
//!
//! The simplification is **answer-set preserving**: `simplify(g)` and `g` have
//! the same stable models (verified against clasp/clingo), but the simplified
//! program is smaller — the point of the exercise.
//!
//! Two fixpoints drive it, both bottom-up (the grounding-as-fixpoint that §5.1
//! assigns to the GPU):
//! - **`poss`** — an over-approximation of the derivable atoms (least model with
//!   negation ignored). An atom outside `poss` is *definitely false*.
//! - **`cert`** — the deterministic consequences (a rule fires for certain when
//!   every positive body atom is certain and every negative one is impossible).
//!   An atom in `cert` is *definitely true*.

use crate::{GRule, Ground, GroundAtom};
use std::collections::BTreeSet;

/// Simplify a grounded program, preserving its answer sets. Returns a new
/// `Ground` sharing the original atom table (ids are stable) with fewer / shorter
/// rules: definitely-false rules dropped, certain facts substituted out, and
/// duplicate + subsumed rules removed.
pub fn simplify(g: &Ground) -> Ground {
    let n = g.n_atoms();
    let poss = possibly_true(g, n);
    let cert = certainly_true(g, n, &poss);

    let mut kept: Vec<GRule> = Vec::new();
    'rules: for r in &g.rules {
        // Drop a rule whose body can never hold: a positive atom that is
        // definitely false, or a negative atom that is definitely true.
        for &p in &r.pos {
            if !poss[p] {
                continue 'rules;
            }
        }
        for &nn in &r.neg {
            if cert[nn] {
                continue 'rules;
            }
        }
        // Fact substitution: drop positive literals known true, and negative
        // literals whose atom is impossible (`not false` = true).
        let pos: Vec<usize> = r.pos.iter().copied().filter(|&p| !cert[p]).collect();
        let neg: Vec<usize> = r.neg.iter().copied().filter(|&nn| poss[nn]).collect();
        kept.push(GRule {
            head: r.head,
            pos,
            neg,
        });
    }

    dedup_and_desubsume(&mut kept);

    Ground {
        rules: kept,
        atoms: g.atoms.clone(),
    }
}

/// Count of atoms mentioned by a rule (for the reduction metric).
fn body_len(r: &GRule) -> usize {
    r.pos.len() + r.neg.len()
}

/// A cheap reduction summary: `(rules_before, rules_after, literals_before,
/// literals_after)`.
pub fn reduction(before: &Ground, after: &Ground) -> (usize, usize, usize, usize) {
    let lit = |g: &Ground| g.rules.iter().map(body_len).sum::<usize>();
    (
        before.rules.len(),
        after.rules.len(),
        lit(before),
        lit(after),
    )
}

// --- fixpoints ---------------------------------------------------------------

/// Least model ignoring negation: an over-approximation of the true atoms.
fn possibly_true(g: &Ground, n: usize) -> Vec<bool> {
    let mut poss = vec![false; n];
    loop {
        let mut changed = false;
        for r in &g.rules {
            if let Some(h) = r.head {
                if !poss[h] && r.pos.iter().all(|&p| poss[p]) {
                    poss[h] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return poss;
        }
    }
}

/// Deterministic consequences: a rule fires for certain when every positive
/// body atom is certain and every negative one is impossible (`!poss`).
fn certainly_true(g: &Ground, n: usize, poss: &[bool]) -> Vec<bool> {
    let mut cert = vec![false; n];
    loop {
        let mut changed = false;
        for r in &g.rules {
            if let Some(h) = r.head {
                if !cert[h] && r.pos.iter().all(|&p| cert[p]) && r.neg.iter().all(|&nn| !poss[nn]) {
                    cert[h] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return cert;
        }
    }
}

// --- dedup + subsumption -----------------------------------------------------

/// A canonical key for exact-duplicate detection.
fn key(r: &GRule) -> (Option<usize>, Vec<usize>, Vec<usize>) {
    let mut pos = r.pos.clone();
    let mut neg = r.neg.clone();
    pos.sort_unstable();
    pos.dedup();
    neg.sort_unstable();
    neg.dedup();
    (r.head, pos, neg)
}

/// Remove exact duplicates, then rules subsumed by a strictly more general rule
/// with the same head (`R1.pos ⊆ R2.pos ∧ R1.neg ⊆ R2.neg` ⇒ drop `R2`).
fn dedup_and_desubsume(rules: &mut Vec<GRule>) {
    // canonicalize + exact dedup
    let mut seen: BTreeSet<(Option<usize>, Vec<usize>, Vec<usize>)> = BTreeSet::new();
    let mut uniq: Vec<GRule> = Vec::new();
    for r in rules.drain(..) {
        let k = key(&r);
        if seen.insert(k.clone()) {
            uniq.push(GRule {
                head: k.0,
                pos: k.1,
                neg: k.2,
            });
        }
    }

    // subsumption: drop R2 if some other R1 (same head) has pos⊆pos, neg⊆neg.
    let subset = |a: &[usize], b: &[usize]| a.iter().all(|x| b.binary_search(x).is_ok());
    let mut keep = vec![true; uniq.len()];
    for i in 0..uniq.len() {
        for j in 0..uniq.len() {
            if i == j || !keep[j] {
                continue;
            }
            // does uniq[j] subsume uniq[i]? (j more general ⇒ drop i)
            if uniq[j].head == uniq[i].head
                && body_len(&uniq[j]) < body_len(&uniq[i])
                && subset(&uniq[j].pos, &uniq[i].pos)
                && subset(&uniq[j].neg, &uniq[i].neg)
            {
                keep[i] = false;
                break;
            }
        }
    }
    *rules = uniq
        .into_iter()
        .zip(keep)
        .filter_map(|(r, k)| k.then_some(r))
        .collect();
}

// --- resolve certain facts (for reporting) -----------------------------------

/// The atoms proven certainly-true by simplification (the deterministic facts).
pub fn certain_facts(g: &Ground) -> Vec<GroundAtom> {
    let n = g.n_atoms();
    let poss = possibly_true(g, n);
    let cert = certainly_true(g, n, &poss);
    (0..n)
        .filter(|&i| cert[i])
        .map(|i| g.atoms[i].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ground, solve, Val};
    use strata_ir::high::program::{atom, Literal, Rule};

    fn a(p: &str) -> strata_ir::high::program::Atom {
        atom(p, vec![])
    }

    /// Simplification must not change the answer sets (reference solve on both).
    fn assert_preserves(rules: &[Rule], facts: &[(String, Vec<Val>)], cons: &[Vec<Literal>]) {
        let g = ground(rules, facts, cons).unwrap();
        let s = simplify(&g);
        // The simplified program is at most as large.
        let (rb, ra, lb, la) = reduction(&g, &s);
        assert!(ra <= rb, "rules grew: {rb}→{ra}");
        assert!(la <= lb, "literals grew: {lb}→{la}");
        // Same answer sets, computed via the reduct oracle on both encodings.
        let m0 = solve(rules, facts, cons).unwrap();
        let m1 = crate::stable_models_of(&s).unwrap();
        assert_eq!(m0, m1, "simplification changed the answer sets");
    }

    #[test]
    fn substitutes_facts_and_shrinks() {
        // p.  q :- p.  r :- q, not s.   p,q,r all certain; s impossible.
        let facts = vec![("p".to_string(), vec![])];
        let rules = vec![
            Rule {
                head: a("q"),
                body: vec![Literal::Pos(a("p"))],
            },
            Rule {
                head: a("r"),
                body: vec![Literal::Pos(a("q")), Literal::Neg(a("s"))],
            },
        ];
        let g = ground(&rules, &facts, &[]).unwrap();
        let s = simplify(&g);
        let (_, _, lb, la) = reduction(&g, &s);
        assert!(la < lb, "expected fewer body literals after substitution");
        assert_preserves(&rules, &facts, &[]);
        // r is a certain fact after simplification.
        let cf = certain_facts(&g);
        assert!(cf.contains(&("r".to_string(), vec![])));
    }

    #[test]
    fn drops_dead_rules() {
        // q :- p.   (no p anywhere) ⇒ q impossible; the rule is dead.
        let rules = vec![Rule {
            head: a("q"),
            body: vec![Literal::Pos(a("p"))],
        }];
        let g = ground(&rules, &[], &[]).unwrap();
        let s = simplify(&g);
        assert!(s.rules.is_empty(), "dead rule should be removed");
        assert_preserves(&rules, &[], &[]);
    }

    #[test]
    fn removes_subsumed_rule() {
        // h :- a.   h :- a, b.   (second subsumed by first) — over ground facts.
        let facts = vec![
            ("a".to_string(), vec![] as Vec<Val>),
            ("b".to_string(), vec![]),
        ];
        let rules = vec![
            Rule {
                head: a("h"),
                body: vec![Literal::Pos(a("a"))],
            },
            Rule {
                head: a("h"),
                body: vec![Literal::Pos(a("a")), Literal::Pos(a("b"))],
            },
        ];
        // After fact-substitution both become `h.`; dedup leaves one.
        let g = ground(&rules, &facts, &[]).unwrap();
        let s = simplify(&g);
        let h_rules = s.rules.iter().filter(|r| r.head.is_some()).count();
        // a, b facts + single h rule (the two h-rules collapse to one `h.`)
        assert!(
            h_rules <= 3,
            "duplicate/subsumed h-rules not collapsed: {h_rules}"
        );
        assert_preserves(&rules, &facts, &[]);
    }

    #[test]
    fn preserves_even_cycle() {
        assert_preserves(
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
}
