//! Probabilistic evaluation — режим B by exact possible-world enumeration.
//! [Phase 4, spec 1.5/2.1, distribution semantics à la ProbLog/Sato]
//!
//! A probabilistic fact `p :: a(x)` is an independent Bernoulli: the atom is
//! present with probability `p`. The marginal probability of a derived tuple is
//! the sum, over all `2^n` worlds (subsets of the `n` probabilistic facts), of
//! the world's probability times whether the tuple is derivable in that world:
//!
//!   P(t) = Σ_W  (∏_{f∈W} p_f)(∏_{f∉W} (1-p_f)) · [t derivable from certain∪W]
//!
//! This is the *obviously-correct* reference (the режим-B analogue of the naive
//! `T_P` oracle): exponential in `n`, but exact even when derivations share
//! facts — the correlation case where a naive semiring convolution over-counts
//! (spec 2.1). Knowledge compilation (SDD/WMC) and top-k are the "fast" methods a
//! later slice must match against this. Bool deduction only.

use std::collections::HashMap;

use strata_ir::core::{CoreProgram, Semiring};

use crate::naive::{run, EvalError};
use crate::value::{Ann, GroundVal};

pub type Tuple = Vec<GroundVal>;
pub type Marginals = HashMap<String, HashMap<Tuple, f64>>;

/// Exact enumeration is refused past this many probabilistic facts (2^n runs).
/// Exact режим B is #P-hard; beyond this a compiled/top-k method is required.
pub const MAX_PROB_FACTS: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub enum ProbError {
    /// More probabilistic facts than exact enumeration allows (spec: #P-hard).
    TooManyProbFacts(usize),
    /// A probability outside [0, 1].
    BadProbability(f64),
    /// The deductive part must be Bool (probabilities annotate Bool relations).
    NotBool(String),
    /// The underlying Bool evaluation failed.
    Eval(EvalError),
}

impl std::fmt::Display for ProbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbError::TooManyProbFacts(n) => write!(
                f,
                "{n} probabilistic facts exceed the exact-enumeration limit of {MAX_PROB_FACTS} \
                 (exact режим B is #P-hard; use knowledge compilation / top-k)"
            ),
            ProbError::BadProbability(p) => write!(f, "probability {p} is outside [0, 1]"),
            ProbError::NotBool(p) => write!(f, "predicate `{p}` is not Bool; режим B is Bool-only"),
            ProbError::Eval(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProbError {}

/// Compute the marginal probability of every derivable tuple of every predicate.
///
/// `certain` facts are always present (probability 1); `prob` facts are present
/// with their given probability. All predicates must be Bool.
pub fn marginals(
    core: &CoreProgram,
    certain: &[(String, Tuple)],
    prob: &[(String, Tuple, f64)],
) -> Result<Marginals, ProbError> {
    if let Some(p) = core
        .predicates
        .iter()
        .find(|p| p.semiring != Semiring::Bool)
    {
        return Err(ProbError::NotBool(p.name.clone()));
    }
    let n = prob.len();
    if n > MAX_PROB_FACTS {
        return Err(ProbError::TooManyProbFacts(n));
    }
    for &(_, _, p) in prob {
        if !(0.0..=1.0).contains(&p) {
            return Err(ProbError::BadProbability(p));
        }
    }

    let mut acc: Marginals = HashMap::new();
    // Enumerate all 2^n worlds via a bitmask over the probabilistic facts.
    for mask in 0u32..(1u32 << n) {
        // World probability = ∏ present p · ∏ absent (1-p).
        let mut w = 1.0f64;
        for (i, &(_, _, p)) in prob.iter().enumerate() {
            w *= if mask & (1 << i) != 0 { p } else { 1.0 - p };
        }
        if w == 0.0 {
            continue; // impossible world contributes nothing
        }

        // EDB = certain facts + the probabilistic facts present in this world.
        let mut edb: Vec<(&str, Tuple, Ann)> = certain
            .iter()
            .map(|(pr, t)| (pr.as_str(), t.clone(), Ann::Unit))
            .collect();
        for (i, (pr, t, _)) in prob.iter().enumerate() {
            if mask & (1 << i) != 0 {
                edb.push((pr.as_str(), t.clone(), Ann::Unit));
            }
        }

        let db = run(core, &edb).map_err(ProbError::Eval)?;
        for pred in db.predicates() {
            let rel = db.relation(pred).unwrap();
            let entry = acc.entry(pred.clone()).or_default();
            for tuple in rel.rows.keys() {
                *entry.entry(tuple.clone()).or_insert(0.0) += w;
            }
        }
    }
    Ok(acc)
}

/// Marginals of one predicate's tuples matching a pattern (`None` = any position).
/// Answers a `?prob pred(pattern)` query; results are sorted by tuple.
pub fn query(
    core: &CoreProgram,
    certain: &[(String, Tuple)],
    prob: &[(String, Tuple, f64)],
    pred: &str,
    pattern: &[Option<GroundVal>],
) -> Result<Vec<(Tuple, f64)>, ProbError> {
    let all = marginals(core, certain, prob)?;
    let mut out: Vec<(Tuple, f64)> = all
        .get(pred)
        .into_iter()
        .flatten()
        .filter(|(t, _)| matches(t, pattern))
        .map(|(t, p)| (t.clone(), *p))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn matches(tuple: &[GroundVal], pattern: &[Option<GroundVal>]) -> bool {
    tuple.len() == pattern.len()
        && tuple
            .iter()
            .zip(pattern)
            .all(|(v, p)| p.is_none_or(|want| want == *v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_ir::core::{CoreAtom, CoreLiteral, CorePred, CoreRule, CoreTerm};
    use strata_ir::dict::SymbolId;

    fn v(slot: u32) -> CoreTerm {
        CoreTerm::Var { slot }
    }
    fn tc_program() -> CoreProgram {
        // path(X,Y) :- edge(X,Y).  path(X,Z) :- edge(X,Y), path(Y,Z).
        let p = |n: &str| CorePred {
            name: n.into(),
            arity: 2,
            semiring: Semiring::Bool,
            stratum: 0,
        };
        let atom = |n: &str, a: u32, b: u32| CoreAtom {
            pred: n.into(),
            args: vec![v(a), v(b)],
        };
        CoreProgram {
            predicates: vec![p("edge"), p("path")],
            rules: vec![
                CoreRule {
                    head: atom("path", 0, 1),
                    body: vec![CoreLiteral::Pos(atom("edge", 0, 1))],
                    stratum: 0,
                    var_count: 2,
                    neg_weight_cycle_check: false,
                },
                CoreRule {
                    head: atom("path", 0, 2),
                    body: vec![
                        CoreLiteral::Pos(atom("edge", 0, 1)),
                        CoreLiteral::Pos(atom("path", 1, 2)),
                    ],
                    stratum: 0,
                    var_count: 3,
                    neg_weight_cycle_check: false,
                },
            ],
            num_strata: 1,
        }
    }
    fn sym(n: u32) -> GroundVal {
        GroundVal::Sym(SymbolId(n))
    }
    // constants: a=0, b=1, c=2
    fn edge(x: u32, y: u32, p: f64) -> (String, Tuple, f64) {
        ("edge".into(), vec![sym(x), sym(y)], p)
    }

    fn prob_of(core: &CoreProgram, prob: &[(String, Tuple, f64)], t: &[u32]) -> f64 {
        let pat: Vec<Option<GroundVal>> = t.iter().map(|&c| Some(sym(c))).collect();
        query(core, &[], prob, "path", &pat)
            .unwrap()
            .first()
            .map(|x| x.1)
            .unwrap_or(0.0)
    }

    #[test]
    fn independent_conjunction() {
        // edge(a,b)=0.5, edge(b,c)=0.5 ⇒ P(path(a,c)) = 0.5·0.5 = 0.25.
        let core = tc_program();
        let prob = vec![edge(0, 1, 0.5), edge(1, 2, 0.5)];
        assert!((prob_of(&core, &prob, &[0, 2]) - 0.25).abs() < 1e-12);
        // P(path(a,b)) = P(edge(a,b)) = 0.5.
        assert!((prob_of(&core, &prob, &[0, 1]) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn correlated_disjunction_is_exact() {
        // Two routes a→c: direct edge(a,c)=0.5, and a→b→c via 0.5,0.5.
        // P = 1 - (1-0.5)(1-0.25) = 0.625.  A naive ⊕/⊗ would over-count.
        let core = tc_program();
        let prob = vec![edge(0, 2, 0.5), edge(0, 1, 0.5), edge(1, 2, 0.5)];
        assert!((prob_of(&core, &prob, &[0, 2]) - 0.625).abs() < 1e-12);
    }

    #[test]
    fn too_many_prob_facts_is_refused() {
        let core = tc_program();
        let prob: Vec<_> = (0..(MAX_PROB_FACTS as u32 + 1))
            .map(|i| edge(i, i + 1, 0.5))
            .collect();
        assert!(matches!(
            marginals(&core, &[], &prob),
            Err(ProbError::TooManyProbFacts(_))
        ));
    }
}
