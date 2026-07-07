//! Provenance capture — режим B stage 1: each derived tuple carries its
//! minimal proof DNF over the probabilistic leaves. [Prov/Prov_k, spec §2.1–2.2]
//!
//! A *proof* is the set of probabilistic EDB facts a derivation rests on,
//! written as signed literals aspif-style: `+(i+1)` means prob fact `i` is
//! present, `-(i+1)` absent (the dual literal `x̄`, with `x·x̄ = 0`). A tuple's
//! provenance is the absorption-minimal antichain of its proofs — dropping a
//! proof that is a superset of another loses nothing (`P(A ∨ (A∧B)) = P(A)`),
//! and minimality is what makes the fixpoint terminate even when a *Bool*
//! predicate recurses over soft facts: a monotone positive program's
//! derivability per world is fixed by its minimal support sets, of which there
//! are finitely many.
//!
//! `Prov_k` predicates additionally prune each tuple to its k best proofs
//! (weight = the proof's own probability, ties broken lexicographically — a
//! total order, so merging is order-invariant, spec §2.2). The kept subset
//! makes any downstream WMC a guaranteed lower bound.
//!
//! Stratified negation over soft provenance takes the complement of the
//! negated tuple's proof DNF (dual literals distributed, `x·x̄ = 0`,
//! absorption) — exact, budgeted like every proof set. The one remaining
//! honest refusal: aggregation over soft-supported bindings (counting
//! correlated worlds); the enumeration oracle in [`crate::prob`] covers it
//! world by world.
//!
//! Compilation (stage 2) and WMC/gradients (stage 3) live in `strata-prob`;
//! this module only captures. Its differential test cross-checks the captured
//! DNFs, brute-force-counted, against [`crate::prob::marginals`].

use std::collections::{BTreeMap, BTreeSet, HashMap};

use strata_ir::core::{CoreLiteral, CoreProgram, CoreRule, Semiring};
use strata_ir::terms::TermTable;

use crate::naive::{is_aggregate_rule, unify, EvalError};
use crate::store::Tuple;
use crate::value::GroundVal;

/// One proof: a set of signed literals over prob-fact indices (±(i+1)).
pub type Proof = BTreeSet<i64>;
/// A tuple's provenance: an absorption-minimal antichain of proofs.
pub type ProofSet = BTreeSet<Proof>;

/// The per-tuple budget for exact capture: a tuple whose minimal proof
/// antichain grows past this is refused with [`ProvError::ProofBudget`] — the
/// declared escape valve is `Prov_k`, not an OOM.
pub const MAX_PROOFS_PER_TUPLE: usize = 10_000;

/// How a predicate's provenance is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvMode {
    /// Keep every minimal proof (exact WMC downstream).
    Exact,
    /// Keep the k best proofs per tuple (lower-bound WMC downstream).
    TopK(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProvError {
    /// Provenance capture is Bool-only (probabilities annotate Bool relations;
    /// Trop is режим A and incomparable with Prov).
    NotBool(String),
    /// A probability outside [0, 1].
    BadProbability(f64),
    /// An aggregate over soft-supported bindings — correlated counting is
    /// refused; aggregates must rest on certain tuples only.
    SoftAggregate(String),
    /// A tuple's minimal proof antichain exceeded the capture budget; exact
    /// provenance refuses the blow-up (`Prov_k` is the declared alternative).
    ProofBudget { pred: String, bound: usize },
    /// The underlying Bool machinery failed (unknown predicate, safety, ...).
    Eval(EvalError),
}

impl std::fmt::Display for ProvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvError::NotBool(p) => {
                write!(f, "predicate `{p}` is not Bool-based; режим B is Bool-only")
            }
            ProvError::BadProbability(p) => write!(f, "probability {p} is outside [0, 1]"),
            ProvError::SoftAggregate(p) => write!(
                f,
                "aggregate over soft-supported `{p}` tuples is not supported by capture; \
                 aggregates must rest on certain evidence"
            ),
            ProvError::ProofBudget { pred, bound } => write!(
                f,
                "`{pred}` accumulated more than {bound} minimal proofs for one tuple; \
                 exact provenance refuses the blow-up — annotate it `Prov_k(k)` for a \
                 declared lower bound"
            ),
            ProvError::Eval(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProvError {}

/// The saturated provenance database: predicate → tuple → its proofs.
#[derive(Debug, Clone, Default)]
pub struct ProvDb {
    pub rels: BTreeMap<String, BTreeMap<Tuple, ProofSet>>,
}

impl ProvDb {
    pub fn relation(&self, pred: &str) -> Option<&BTreeMap<Tuple, ProofSet>> {
        self.rels.get(pred)
    }

    /// Tuples of `pred` matching `pattern` (`None` = any), with their proofs as
    /// sorted literal vectors — the shape `strata-prob` compiles. Sorted.
    pub fn query(&self, pred: &str, pattern: &[Option<GroundVal>]) -> Vec<(Tuple, Vec<Vec<i64>>)> {
        self.rels
            .get(pred)
            .into_iter()
            .flatten()
            .filter(|(t, _)| {
                t.len() == pattern.len()
                    && t.iter()
                        .zip(pattern)
                        .all(|(v, p)| p.is_none_or(|want| want == *v))
            })
            .map(|(t, ps)| {
                (
                    t.clone(),
                    ps.iter().map(|p| p.iter().copied().collect()).collect(),
                )
            })
            .collect()
    }
}

/// Insert `proof` into `set` keeping it absorption-minimal. Returns whether the
/// set changed.
pub(crate) fn insert_minimal(set: &mut ProofSet, proof: Proof) -> bool {
    if set.iter().any(|kept| kept.is_subset(&proof)) {
        return false;
    }
    set.retain(|kept| !proof.is_subset(kept));
    set.insert(proof);
    true
}

/// The DNF product `acc ⊗ other`: every pairwise union, contradictions
/// (`x·x̄ = 0`) dropped, result minimized.
fn product(acc: &ProofSet, other: &ProofSet) -> ProofSet {
    let mut out = ProofSet::new();
    for a in acc {
        for b in other {
            let joined: Proof = a.union(b).copied().collect();
            if joined.iter().all(|&l| !joined.contains(&(-l))) {
                insert_minimal(&mut out, joined);
            }
        }
    }
    out
}

/// The complement `¬(p₁ ∨ ... ∨ pₙ)` of a proof DNF, as a proof DNF:
/// distribute `⊗ᵢ ¬pᵢ` where `¬(l₁ ∧ ... ∧ lₖ)` is the DNF of the negated
/// literals `{¬l₁} ∨ ... ∨ {¬lₖ}` (dual literals, `x·x̄ = 0` drops
/// contradictions, absorption keeps the antichain minimal). Worst case is
/// exponential — the same honest bill as everywhere in режим B — so the
/// per-tuple proof budget applies here too.
fn complement(proofs: &ProofSet, max_proofs: usize) -> Option<ProofSet> {
    let mut acc = ProofSet::from([Proof::new()]);
    for p in proofs {
        let negated: ProofSet = p.iter().map(|&l| Proof::from([-l])).collect();
        acc = product(&acc, &negated);
        if acc.len() > max_proofs {
            return None; // budget: the caller reports ProofBudget
        }
    }
    Some(acc)
}

/// The weight of one proof under the leaf probabilities.
fn proof_weight(proof: &Proof, prob: &[(String, Tuple, f64)]) -> f64 {
    proof
        .iter()
        .map(|&l| {
            let p = prob[(l.abs() - 1) as usize].2;
            if l > 0 {
                p
            } else {
                1.0 - p
            }
        })
        .product()
}

/// Prune a tuple's proofs to the k best by (weight desc, literals asc) — the
/// same total order as `strata_prob::top_k_signed`, so the two agree.
pub(crate) fn prune_top_k(set: &mut ProofSet, k: usize, prob: &[(String, Tuple, f64)]) {
    if set.len() <= k {
        return;
    }
    let mut scored: Vec<(f64, Proof)> = set
        .iter()
        .map(|p| (proof_weight(p, prob), p.clone()))
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    scored.truncate(k);
    *set = scored.into_iter().map(|(_, p)| p).collect();
}

/// Run the program to its provenance fixpoint. `certain` facts carry the empty
/// proof (true); `prob` fact `i` carries the single proof `{+(i+1)}`. `modes`
/// gives the `Prov_k` bounds (absent predicates capture exactly).
pub fn run_prov(
    core: &CoreProgram,
    certain: &[(String, Tuple)],
    prob: &[(String, Tuple, f64)],
    modes: &HashMap<String, ProvMode>,
    terms: &mut TermTable,
) -> Result<ProvDb, ProvError> {
    run_prov_with_budget(core, certain, prob, modes, MAX_PROOFS_PER_TUPLE, terms)
}

/// [`run_prov`] with an explicit per-tuple proof budget (tests use tiny ones).
pub fn run_prov_with_budget(
    core: &CoreProgram,
    certain: &[(String, Tuple)],
    prob: &[(String, Tuple, f64)],
    modes: &HashMap<String, ProvMode>,
    max_proofs_per_tuple: usize,
    terms: &mut TermTable,
) -> Result<ProvDb, ProvError> {
    if let Some(p) = core
        .predicates
        .iter()
        .find(|p| p.semiring != Semiring::Bool)
    {
        return Err(ProvError::NotBool(p.name.clone()));
    }
    for &(_, _, p) in prob {
        if !(0.0..=1.0).contains(&p) {
            return Err(ProvError::BadProbability(p));
        }
    }

    let mut db = ProvDb::default();
    for p in &core.predicates {
        db.rels.entry(p.name.clone()).or_default();
    }
    let known = |pred: &str, db: &ProvDb| -> Result<(), ProvError> {
        if db.rels.contains_key(pred) {
            Ok(())
        } else {
            Err(ProvError::Eval(EvalError::UnknownPred(pred.to_string())))
        }
    };
    for (pred, tuple) in certain {
        known(pred, &db)?;
        let set = db
            .rels
            .get_mut(pred)
            .unwrap()
            .entry(tuple.clone())
            .or_default();
        insert_minimal(set, Proof::new());
    }
    for (i, (pred, tuple, _)) in prob.iter().enumerate() {
        known(pred, &db)?;
        let set = db
            .rels
            .get_mut(pred)
            .unwrap()
            .entry(tuple.clone())
            .or_default();
        insert_minimal(set, Proof::from([(i as i64) + 1]));
    }

    saturate(&mut db, core, prob, modes, max_proofs_per_tuple, terms)?;
    Ok(db)
}

/// Run the capture fixpoint over `db`'s current contents until stable —
/// the body of [`run_prov`], reusable by the incremental maintainer to resume
/// after inserting a leaf or repopulate top-k sets after deleting one.
pub(crate) fn saturate(
    db: &mut ProvDb,
    core: &CoreProgram,
    prob: &[(String, Tuple, f64)],
    modes: &HashMap<String, ProvMode>,
    max_proofs_per_tuple: usize,
    terms: &mut TermTable,
) -> Result<(), ProvError> {
    let pred_stratum: HashMap<String, u32> = core
        .predicates
        .iter()
        .map(|p| (p.name.clone(), p.stratum))
        .collect();

    for stratum in 0..core.num_strata {
        // Aggregates first, once (their bodies are strictly lower — CHECK-9).
        for rule in core
            .rules_in_stratum(stratum)
            .filter(|r| is_aggregate_rule(r))
        {
            eval_aggregate(db, core, rule, &pred_stratum, stratum, terms)?;
        }

        loop {
            let mut derived: Vec<(String, Tuple, ProofSet)> = Vec::new();
            for rule in core
                .rules_in_stratum(stratum)
                .filter(|r| !is_aggregate_rule(r))
            {
                let mut binding = vec![None; rule.var_count as usize];
                let mut acc = ProofSet::from([Proof::new()]);
                solve_prov(
                    db,
                    &pred_stratum,
                    stratum,
                    terms,
                    rule,
                    0,
                    &mut binding,
                    &mut acc,
                    &mut derived,
                    max_proofs_per_tuple,
                )
                .map_err(EvalErrorOrProv::into_prov)?;
            }
            let mut changed = false;
            for (pred, tuple, proofs) in derived {
                let topk = match modes.get(&pred) {
                    Some(ProvMode::TopK(k)) => Some(*k as usize),
                    _ => None,
                };
                let set = db.rels.get_mut(&pred).unwrap().entry(tuple).or_default();
                // Under a top-k bound, "changed" must compare the *pruned*
                // result against the pre-insert state: re-deriving a proof the
                // prune keeps dropping is not progress (else no fixpoint).
                let before = topk.map(|_| set.clone());
                let mut local = false;
                for proof in proofs {
                    local |= insert_minimal(set, proof);
                }
                if let (true, Some(k)) = (local, topk) {
                    prune_top_k(set, k, prob);
                    local = Some(&*set) != before.as_ref();
                } else if local && set.len() > max_proofs_per_tuple {
                    // Exact capture past the budget: refuse, name the valve.
                    return Err(ProvError::ProofBudget {
                        pred,
                        bound: max_proofs_per_tuple,
                    });
                }
                changed |= local;
            }
            if !changed {
                break;
            }
        }
    }
    Ok(())
}

/// Solve one rule body left-to-right, threading the proof-DNF product.
#[allow(clippy::too_many_arguments)]
fn solve_prov(
    db: &ProvDb,
    pred_stratum: &HashMap<String, u32>,
    cur_stratum: u32,
    terms: &mut TermTable,
    rule: &CoreRule,
    idx: usize,
    binding: &mut [Option<GroundVal>],
    acc: &mut ProofSet,
    out: &mut Vec<(String, Tuple, ProofSet)>,
    max_proofs: usize,
) -> Result<(), EvalErrorOrProv> {
    if idx == rule.body.len() {
        // Heads may construct compound terms; they intern into the program's
        // shared table (depth-bounded → dropped derivation, sound-incomplete).
        if let Some(tuple) =
            crate::naive::build_head(&rule.head, binding, terms).map_err(EvalErrorOrProv::eval)?
        {
            out.push((rule.head.pred.clone(), tuple, acc.clone()));
        }
        return Ok(());
    }
    match &rule.body[idx] {
        CoreLiteral::Pos(atom) => {
            let rel = db
                .rels
                .get(&atom.pred)
                .ok_or_else(|| EvalErrorOrProv::eval(EvalError::UnknownPred(atom.pred.clone())))?;
            let rows: Vec<(Tuple, ProofSet)> =
                rel.iter().map(|(t, ps)| (t.clone(), ps.clone())).collect();
            for (tuple, proofs) in rows {
                let mut b = binding.to_vec();
                if unify(&atom.args, &tuple, &mut b, terms) {
                    let mut next = product(acc, &proofs);
                    if next.is_empty() {
                        continue; // contradictory support: x·x̄ = 0
                    }
                    // Budget the *intermediate* DNF too: the blow-up must be
                    // refused while it is happening, not after the whole batch
                    // has been materialized.
                    if next.len() > max_proofs {
                        return Err(EvalErrorOrProv::Prov(ProvError::ProofBudget {
                            pred: rule.head.pred.clone(),
                            bound: max_proofs,
                        }));
                    }
                    solve_prov(
                        db,
                        pred_stratum,
                        cur_stratum,
                        terms,
                        rule,
                        idx + 1,
                        &mut b,
                        &mut next,
                        out,
                        max_proofs,
                    )?;
                }
            }
            Ok(())
        }
        CoreLiteral::Neg(atom) => {
            let ns = pred_stratum
                .get(&atom.pred)
                .ok_or_else(|| EvalErrorOrProv::eval(EvalError::UnknownPred(atom.pred.clone())))?;
            if *ns >= cur_stratum {
                return Err(EvalErrorOrProv::eval(EvalError::NegationNotStratified {
                    pred: atom.pred.clone(),
                }));
            }
            let Some(tuple) =
                crate::naive::build_head(atom, binding, terms).map_err(EvalErrorOrProv::eval)?
            else {
                // A depth-bounded term cannot be present: the negation holds.
                return solve_prov(
                    db,
                    pred_stratum,
                    cur_stratum,
                    terms,
                    rule,
                    idx + 1,
                    binding,
                    acc,
                    out,
                    max_proofs,
                );
            };
            let rel = db
                .rels
                .get(&atom.pred)
                .ok_or_else(|| EvalErrorOrProv::eval(EvalError::UnknownPred(atom.pred.clone())))?;
            match rel.get(&tuple) {
                // Absent: the negation holds certainly.
                None => solve_prov(
                    db,
                    pred_stratum,
                    cur_stratum,
                    terms,
                    rule,
                    idx + 1,
                    binding,
                    acc,
                    out,
                    max_proofs,
                ),
                Some(proofs) => {
                    if proofs.contains(&Proof::new()) {
                        // Certainly present: the negation fails, prune.
                        return Ok(());
                    }
                    // Derived soft provenance: the absence of the atom is the
                    // complement of its proof DNF — dual literals distributed,
                    // budgeted like every other proof set.
                    let Some(negated) = complement(proofs, max_proofs) else {
                        return Err(EvalErrorOrProv::Prov(ProvError::ProofBudget {
                            pred: atom.pred.clone(),
                            bound: max_proofs,
                        }));
                    };
                    let mut next = product(acc, &negated);
                    if next.is_empty() {
                        return Ok(()); // contradictory: the tuple is certain here
                    }
                    if next.len() > max_proofs {
                        return Err(EvalErrorOrProv::Prov(ProvError::ProofBudget {
                            pred: rule.head.pred.clone(),
                            bound: max_proofs,
                        }));
                    }
                    solve_prov(
                        db,
                        pred_stratum,
                        cur_stratum,
                        terms,
                        rule,
                        idx + 1,
                        binding,
                        &mut next,
                        out,
                        max_proofs,
                    )
                }
            }
        }
    }
}

/// Aggregate rules under capture: allowed only when every body relation is
/// entirely certain — then the fold happens in one world and the result is
/// certain too. Delegates to the Bool aggregate machinery on that slice.
fn eval_aggregate(
    db: &mut ProvDb,
    core: &CoreProgram,
    rule: &CoreRule,
    pred_stratum: &HashMap<String, u32>,
    cur: u32,
    terms: &mut TermTable,
) -> Result<(), ProvError> {
    for lit in &rule.body {
        let (CoreLiteral::Pos(a) | CoreLiteral::Neg(a)) = lit;
        let s = pred_stratum
            .get(&a.pred)
            .ok_or_else(|| ProvError::Eval(EvalError::UnknownPred(a.pred.clone())))?;
        if *s >= cur {
            return Err(ProvError::Eval(EvalError::AggregateInRecursion {
                pred: a.pred.clone(),
            }));
        }
        // A soft tuple in any body relation (positive: world-dependent count;
        // negated: world-dependent membership) is a refusal.
        if let Some(rel) = db.rels.get(&a.pred) {
            if rel.values().any(|ps| !ps.contains(&Proof::new())) {
                return Err(ProvError::SoftAggregate(a.pred.clone()));
            }
        }
    }

    // Project the (all-certain) body relations into a Bool Db, fold there, and
    // read the head tuples back as certain.
    let mut bool_db = crate::store::Db::from_program(core);
    for (pred, rel) in &db.rels {
        for tuple in rel.keys() {
            bool_db.insert(pred, tuple.clone(), crate::value::Ann::Unit);
        }
    }
    crate::naive::eval_aggregate_rule(&mut bool_db, rule, pred_stratum, cur, terms)
        .map_err(ProvError::Eval)?;
    let head_rel = bool_db
        .relation(&rule.head.pred)
        .ok_or_else(|| ProvError::Eval(EvalError::UnknownPred(rule.head.pred.clone())))?;
    let target = db.rels.get_mut(&rule.head.pred).unwrap();
    for tuple in head_rel.rows.keys() {
        insert_minimal(target.entry(tuple.clone()).or_default(), Proof::new());
    }
    Ok(())
}

/// Internal error plumbing: `solve_prov` can fail Bool-wise or Prov-wise.
#[derive(Debug)]
enum EvalErrorOrProv {
    Eval(EvalError),
    Prov(ProvError),
}

impl EvalErrorOrProv {
    fn eval(e: EvalError) -> Self {
        EvalErrorOrProv::Eval(e)
    }
    fn into_prov(self) -> ProvError {
        match self {
            EvalErrorOrProv::Eval(e) => ProvError::Eval(e),
            EvalErrorOrProv::Prov(p) => p,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prob;
    use strata_ir::core::{CoreAtom, CorePred, CoreTerm};
    use strata_ir::dict::SymbolId;

    fn v(slot: u32) -> CoreTerm {
        CoreTerm::Var { slot }
    }
    fn sym(n: u32) -> GroundVal {
        GroundVal::Sym(SymbolId(n))
    }
    fn bpred(n: &str, arity: u32, stratum: u32) -> CorePred {
        CorePred {
            name: n.into(),
            arity,
            semiring: Semiring::Bool,
            stratum,
        }
    }
    fn atom2(n: &str, a: u32, b: u32) -> CoreAtom {
        CoreAtom {
            pred: n.into(),
            args: vec![v(a), v(b)],
        }
    }

    /// path(X,Y) :- edge(X,Y). path(X,Z) :- edge(X,Y), path(Y,Z).
    fn tc_program() -> CoreProgram {
        CoreProgram {
            predicates: vec![bpred("edge", 2, 0), bpred("path", 2, 0)],
            rules: vec![
                CoreRule {
                    head: atom2("path", 0, 1),
                    body: vec![CoreLiteral::Pos(atom2("edge", 0, 1))],
                    stratum: 0,
                    var_count: 2,
                    neg_weight_cycle_check: false,
                },
                CoreRule {
                    head: atom2("path", 0, 2),
                    body: vec![
                        CoreLiteral::Pos(atom2("edge", 0, 1)),
                        CoreLiteral::Pos(atom2("path", 1, 2)),
                    ],
                    stratum: 0,
                    var_count: 3,
                    neg_weight_cycle_check: false,
                },
            ],
            num_strata: 1,
        }
    }

    fn edge(x: u32, y: u32, p: f64) -> (String, Tuple, f64) {
        ("edge".into(), vec![sym(x), sym(y)], p)
    }

    /// Brute-force P(∨ proofs) over the prob facts — the counting oracle.
    fn brute(proofs: &[Vec<i64>], prob: &[(String, Tuple, f64)]) -> f64 {
        let n = prob.len();
        let mut total = 0.0;
        for mask in 0u32..(1u32 << n) {
            let w: f64 = (0..n)
                .map(|i| {
                    if mask & (1 << i) != 0 {
                        prob[i].2
                    } else {
                        1.0 - prob[i].2
                    }
                })
                .product();
            let sat = proofs.iter().any(|proof| {
                proof.iter().all(|&l| {
                    let present = mask & (1 << ((l.abs() - 1) as u32)) != 0;
                    if l > 0 {
                        present
                    } else {
                        !present
                    }
                })
            });
            if sat {
                total += w;
            }
        }
        total
    }

    /// The differential check: capture + brute counting == world enumeration,
    /// for every derived tuple of every predicate.
    fn assert_capture_matches_enumeration(
        core: &CoreProgram,
        certain: &[(String, Tuple)],
        prob: &[(String, Tuple, f64)],
    ) {
        let dbp = run_prov(core, certain, prob, &HashMap::new(), &mut TermTable::new(0))
            .expect("capture runs");
        let m =
            prob::marginals(core, certain, prob, &mut TermTable::new(0)).expect("enumeration runs");
        for (pred, rel) in &dbp.rels {
            for tuple in rel.keys() {
                let proofs = dbp.query(pred, &vec![None; tuple.len()]);
                let (_, ps) = proofs.iter().find(|(t, _)| t == tuple).unwrap();
                let got = brute(ps, prob);
                let want = m
                    .get(pred)
                    .and_then(|r| r.get(tuple))
                    .copied()
                    .unwrap_or(0.0);
                assert!(
                    (got - want).abs() < 1e-9,
                    "{pred}{tuple:?}: capture {got} vs enumeration {want} (proofs {ps:?})"
                );
            }
        }
        // And nothing derivable is missing: every enumerated tuple was captured.
        for (pred, rel) in &m {
            for tuple in rel.keys() {
                assert!(
                    dbp.rels[pred].contains_key(tuple),
                    "{pred}{tuple:?} enumerated but not captured"
                );
            }
        }
    }

    #[test]
    fn correlated_disjunction_matches_enumeration() {
        // Two routes a→c sharing nothing, plus the shared-prefix route set.
        let core = tc_program();
        let prob = vec![edge(0, 2, 0.5), edge(0, 1, 0.5), edge(1, 2, 0.5)];
        assert_capture_matches_enumeration(&core, &[], &prob);
    }

    #[test]
    fn cyclic_soft_graph_terminates_and_matches() {
        // A soft cycle a→b→c→a plus a chord: infinitely many derivation trees,
        // finitely many minimal proofs — absorption terminates the fixpoint.
        let core = tc_program();
        let prob = vec![
            edge(0, 1, 0.9),
            edge(1, 2, 0.8),
            edge(2, 0, 0.7),
            edge(1, 0, 0.6),
        ];
        assert_capture_matches_enumeration(&core, &[], &prob);
    }

    #[test]
    fn mixed_certain_and_soft_matches() {
        let core = tc_program();
        let certain = vec![("edge".to_string(), vec![sym(0), sym(1)])];
        let prob = vec![edge(1, 2, 0.4), edge(0, 2, 0.3)];
        assert_capture_matches_enumeration(&core, &certain, &prob);
        // path(a,b) rests on a certain edge: its only proof is empty.
        let dbp = run_prov(
            &core,
            &certain,
            &prob,
            &HashMap::new(),
            &mut TermTable::new(0),
        )
        .unwrap();
        let ps = &dbp.rels["path"][&vec![sym(0), sym(1)]];
        assert_eq!(ps.len(), 1);
        assert!(ps.contains(&Proof::new()));
    }

    /// q(X) :- node(X), not flag(X). — negation over a soft EDB fact.
    fn neg_program() -> CoreProgram {
        let a1 = |n: &str| CoreAtom {
            pred: n.into(),
            args: vec![v(0)],
        };
        CoreProgram {
            predicates: vec![bpred("node", 1, 0), bpred("flag", 1, 0), {
                let mut p = bpred("q", 1, 1);
                p.arity = 1;
                p
            }],
            rules: vec![CoreRule {
                head: a1("q"),
                body: vec![CoreLiteral::Pos(a1("node")), CoreLiteral::Neg(a1("flag"))],
                stratum: 1,
                var_count: 1,
                neg_weight_cycle_check: false,
            }],
            num_strata: 2,
        }
    }

    #[test]
    fn negated_soft_edb_fact_becomes_a_dual_literal() {
        let core = neg_program();
        let certain = vec![("node".to_string(), vec![sym(0)])];
        let prob = vec![("flag".to_string(), vec![sym(0)], 0.3)];
        let dbp = run_prov(
            &core,
            &certain,
            &prob,
            &HashMap::new(),
            &mut TermTable::new(0),
        )
        .unwrap();
        let ps = &dbp.rels["q"][&vec![sym(0)]];
        assert_eq!(ps.iter().next().unwrap(), &Proof::from([-1i64]));
        assert_capture_matches_enumeration(&core, &certain, &prob);
    }

    #[test]
    fn negation_over_derived_soft_provenance_is_the_dnf_complement() {
        // flag is *derived* from two soft sources: ¬flag = ¬s1 ∨ ¬s2 as a
        // proof DNF ({−1}, {−2}); the marginal must match world enumeration.
        let a1 = |n: &str| CoreAtom {
            pred: n.into(),
            args: vec![v(0)],
        };
        let core = CoreProgram {
            predicates: vec![
                bpred("node", 1, 0),
                bpred("s1", 1, 0),
                bpred("s2", 1, 0),
                bpred("flag", 1, 0),
                bpred("q", 1, 1),
            ],
            rules: vec![
                CoreRule {
                    head: a1("flag"),
                    body: vec![CoreLiteral::Pos(a1("s1")), CoreLiteral::Pos(a1("s2"))],
                    stratum: 0,
                    var_count: 1,
                    neg_weight_cycle_check: false,
                },
                CoreRule {
                    head: a1("q"),
                    body: vec![CoreLiteral::Pos(a1("node")), CoreLiteral::Neg(a1("flag"))],
                    stratum: 1,
                    var_count: 1,
                    neg_weight_cycle_check: false,
                },
            ],
            num_strata: 2,
        };
        let certain = vec![("node".to_string(), vec![sym(0)])];
        let prob = vec![
            ("s1".to_string(), vec![sym(0)], 0.6),
            ("s2".to_string(), vec![sym(0)], 0.7),
        ];
        // P(q) = 1 − P(s1 ∧ s2) = 1 − 0.42 = 0.58; and the full differential.
        assert_capture_matches_enumeration(&core, &certain, &prob);
        let dbp = run_prov(
            &core,
            &certain,
            &prob,
            &HashMap::new(),
            &mut TermTable::new(0),
        )
        .unwrap();
        let ps = &dbp.rels["q"][&vec![sym(0)]];
        assert_eq!(ps.len(), 2, "¬s1 ∨ ¬s2: two dual-literal proofs");
        assert!((brute(&dbp.query("q", &[None])[0].1, &prob) - 0.58).abs() < 1e-12);
    }

    #[test]
    fn topk_prunes_to_a_lower_bound_monotone_in_k() {
        // Many routes a→d; the pruned proof set must under-count, monotonically.
        let core = tc_program();
        let prob = vec![
            edge(0, 3, 0.3),
            edge(0, 1, 0.9),
            edge(1, 3, 0.5),
            edge(0, 2, 0.8),
            edge(2, 3, 0.6),
        ];
        let target = vec![sym(0), sym(3)];
        let exact = {
            let dbp = run_prov(&core, &[], &prob, &HashMap::new(), &mut TermTable::new(0)).unwrap();
            brute(
                &dbp.query("path", &[Some(sym(0)), Some(sym(3))])[0].1,
                &prob,
            )
        };
        let mut prev = 0.0;
        for k in 1..=3 {
            let modes = HashMap::from([("path".to_string(), ProvMode::TopK(k))]);
            let dbp = run_prov(&core, &[], &prob, &modes, &mut TermTable::new(0)).unwrap();
            let ps = &dbp.rels["path"][&target];
            assert!(ps.len() <= k as usize, "k={k}: kept {}", ps.len());
            let lb = brute(
                &dbp.query("path", &[Some(sym(0)), Some(sym(3))])[0].1,
                &prob,
            );
            assert!(lb <= exact + 1e-12, "k={k}: {lb} > exact {exact}");
            assert!(lb + 1e-12 >= prev, "k={k}: {lb} < previous {prev}");
            prev = lb;
        }
        // k big enough covers every minimal proof → exact.
        let modes = HashMap::from([("path".to_string(), ProvMode::TopK(64))]);
        let dbp = run_prov(&core, &[], &prob, &modes, &mut TermTable::new(0)).unwrap();
        let full = brute(
            &dbp.query("path", &[Some(sym(0)), Some(sym(3))])[0].1,
            &prob,
        );
        assert!((full - exact).abs() < 1e-12);
    }

    #[test]
    fn soft_aggregate_is_refused_certain_is_folded() {
        use strata_ir::high::program::AggOp;
        // deg(X, count<Y>) :- edge(X, Y).
        let core = CoreProgram {
            predicates: vec![bpred("edge", 2, 0), bpred("deg", 2, 1)],
            rules: vec![CoreRule {
                head: CoreAtom {
                    pred: "deg".into(),
                    args: vec![
                        v(0),
                        CoreTerm::Agg {
                            op: AggOp::Count,
                            slot: 1,
                        },
                    ],
                },
                body: vec![CoreLiteral::Pos(atom2("edge", 0, 1))],
                stratum: 1,
                var_count: 2,
                neg_weight_cycle_check: false,
            }],
            num_strata: 2,
        };
        // Certain edges fold fine and come out certain.
        let certain = vec![
            ("edge".to_string(), vec![sym(0), sym(1)]),
            ("edge".to_string(), vec![sym(0), sym(2)]),
        ];
        let dbp = run_prov(
            &core,
            &certain,
            &[],
            &HashMap::new(),
            &mut TermTable::new(0),
        )
        .unwrap();
        let ps = &dbp.rels["deg"][&vec![sym(0), GroundVal::Int(2)]];
        assert!(ps.contains(&Proof::new()));
        // A soft edge in the aggregate body is refused.
        let err = run_prov(
            &core,
            &certain,
            &[edge(0, 3, 0.5)],
            &HashMap::new(),
            &mut TermTable::new(0),
        )
        .unwrap_err();
        assert!(matches!(err, ProvError::SoftAggregate(p) if p == "edge"));
    }

    #[test]
    fn proof_budget_refuses_the_blowup_and_names_the_valve() {
        // Many parallel routes a→d: with a tiny per-tuple budget the exact
        // capture refuses; the same program under Prov_k pruning stays fine.
        let core = tc_program();
        let prob: Vec<_> = (1..=4)
            .flat_map(|m| vec![edge(0, m, 0.5), edge(m, 5, 0.5)])
            .collect();
        let err = run_prov_with_budget(
            &core,
            &[],
            &prob,
            &HashMap::new(),
            2,
            &mut TermTable::new(0),
        )
        .unwrap_err();
        match &err {
            ProvError::ProofBudget { pred, bound } => {
                assert_eq!(pred, "path");
                assert_eq!(*bound, 2);
                assert!(err.to_string().contains("Prov_k"), "{err}");
            }
            other => panic!("expected ProofBudget, got {other:?}"),
        }
        let modes = HashMap::from([("path".to_string(), ProvMode::TopK(2))]);
        run_prov_with_budget(&core, &[], &prob, &modes, 2, &mut TermTable::new(0))
            .expect("Prov_k pruning keeps the same program inside the budget");
    }

    #[test]
    fn beyond_the_enumeration_limit_capture_still_runs() {
        // A 25-edge soft chain: 2^25 worlds is past MAX_PROB_FACTS, but the
        // single minimal proof per path tuple is trivial to capture.
        let core = tc_program();
        let prob: Vec<_> = (0..25).map(|i| edge(i, i + 1, 0.9)).collect();
        assert!(
            prob::marginals(&core, &[], &prob, &mut TermTable::new(0)).is_err(),
            "oracle refuses"
        );
        let dbp = run_prov(&core, &[], &prob, &HashMap::new(), &mut TermTable::new(0)).unwrap();
        let ps = &dbp.rels["path"][&vec![sym(0), sym(25)]];
        assert_eq!(ps.len(), 1, "one minimal proof for the full chain");
        assert_eq!(ps.iter().next().unwrap().len(), 25);
    }

    #[test]
    fn fuzz_negation_over_soft_reachability_matches_enumeration() {
        // reach1(Y) :- edge(c0, Y).  reach1(Z) :- reach1(Y), edge(Y, Z).
        // unreach(Y) :- node(Y), not reach1(Y).   — negation over a *derived*
        // soft predicate, per random soft digraph, against the world oracle.
        let c0 = CoreTerm::Const { sym: SymbolId(0) };
        let a1 = |n: &str, slot: u32| CoreAtom {
            pred: n.into(),
            args: vec![v(slot)],
        };
        let core = CoreProgram {
            predicates: vec![
                bpred("node", 1, 0),
                bpred("edge", 2, 0),
                bpred("reach1", 1, 0),
                bpred("unreach", 1, 1),
            ],
            rules: vec![
                CoreRule {
                    head: a1("reach1", 0),
                    body: vec![CoreLiteral::Pos(CoreAtom {
                        pred: "edge".into(),
                        args: vec![c0.clone(), v(0)],
                    })],
                    stratum: 0,
                    var_count: 1,
                    neg_weight_cycle_check: false,
                },
                CoreRule {
                    head: a1("reach1", 1),
                    body: vec![
                        CoreLiteral::Pos(a1("reach1", 0)),
                        CoreLiteral::Pos(atom2("edge", 0, 1)),
                    ],
                    stratum: 0,
                    var_count: 2,
                    neg_weight_cycle_check: false,
                },
                CoreRule {
                    head: a1("unreach", 0),
                    body: vec![
                        CoreLiteral::Pos(a1("node", 0)),
                        CoreLiteral::Neg(a1("reach1", 0)),
                    ],
                    stratum: 1,
                    var_count: 1,
                    neg_weight_cycle_check: false,
                },
            ],
            num_strata: 2,
        };
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut rng = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _case in 0..40 {
            let nodes = 3 + (rng() % 3) as u32;
            let mut certain: Vec<(String, Tuple)> = (0..nodes)
                .map(|i| ("node".to_string(), vec![sym(i)]))
                .collect();
            let mut seen = std::collections::HashSet::new();
            let mut prob = Vec::new();
            for _ in 0..(2 + (rng() % 6) as usize) {
                let x = (rng() % nodes as u64) as u32;
                let y = (rng() % nodes as u64) as u32;
                if x == y || !seen.insert((x, y)) {
                    continue;
                }
                if rng() % 5 == 0 {
                    certain.push(("edge".to_string(), vec![sym(x), sym(y)]));
                } else {
                    let p = (rng() % 1001) as f64 / 1000.0;
                    prob.push(edge(x, y, p));
                }
            }
            assert_capture_matches_enumeration(&core, &certain, &prob);
        }
    }

    #[test]
    fn fuzz_capture_matches_enumeration_on_random_graphs() {
        // Deterministic xorshift sweep: random soft digraphs (≤5 nodes,
        // ≤9 edges, some certain), full differential against enumeration.
        let mut seed = 0x243f6a8885a308d3u64;
        let mut rng = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let core = tc_program();
        for _case in 0..60 {
            let nodes = 3 + (rng() % 3) as u32;
            let n_edges = 2 + (rng() % 8) as usize;
            let mut seen = std::collections::HashSet::new();
            let mut certain = Vec::new();
            let mut prob = Vec::new();
            for _ in 0..n_edges {
                let x = (rng() % nodes as u64) as u32;
                let y = (rng() % nodes as u64) as u32;
                if x == y || !seen.insert((x, y)) {
                    continue; // no self-loops, no duplicates (one leaf per fact)
                }
                if rng() % 4 == 0 {
                    certain.push(("edge".to_string(), vec![sym(x), sym(y)]));
                } else {
                    let p = (rng() % 1001) as f64 / 1000.0;
                    prob.push(edge(x, y, p));
                }
            }
            assert_capture_matches_enumeration(&core, &certain, &prob);
        }
    }
}
