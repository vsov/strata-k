//! Incremental provenance maintenance — soft facts arrive and leave, proof
//! DNFs update without recapturing from scratch. [спека §6, режим-B слой]
//!
//! The leaf-level proof representation makes this exact and simple for the
//! **positive** fragment:
//!
//! - **Insert** a soft fact: append a leaf, seed its `{+leaf}` proof, resume
//!   the capture fixpoint ([`saturate`]) — capture is monotone, so finishing
//!   the fixpoint from the current state is the whole job.
//! - **Delete** a soft fact: drop every proof that uses the leaf. A minimal
//!   proof that avoids the leaf is *already* in the antichain (absorption only
//!   ever removed supersets, and a superset of a leaf-free proof cannot
//!   contain the leaf), so for exact capture the survivors are complete as-is;
//!   a `Prov_k` predicate resumes the fixpoint to refill its pruned top-k.
//!
//! Negation is where insertion stops being monotone (a new soft fact can
//! *weaken* a certain conclusion into a complemented one), so a program whose
//! rules negate anything is refused at construction — the honest boundary;
//! full recapture covers it. Aggregates ride along: they are certain-only and
//! soft edits never touch the certain slice.
//!
//! Verified by fuzz against full recapture after every step of random edit
//! scripts.

use std::collections::HashMap;

use strata_ir::core::{CoreLiteral, CoreProgram};
use strata_ir::terms::TermTable;

use crate::provenance::{
    insert_minimal, prune_top_k, saturate, Proof, ProvDb, ProvError, ProvMode, MAX_PROOFS_PER_TUPLE,
};
use crate::store::Tuple;

/// The incremental maintainer: owns the saturated provenance database and the
/// soft-fact list, and keeps them consistent under inserts and deletes.
pub struct IncProv {
    core: CoreProgram,
    modes: HashMap<String, ProvMode>,
    certain: Vec<(String, Tuple)>,
    /// The leaf space. Deleted leaves are tombstoned (`alive = false`) so
    /// surviving proofs keep their indices; weights of dead leaves are never
    /// read again (no surviving proof mentions them).
    prob: Vec<(String, Tuple, f64)>,
    alive: Vec<bool>,
    db: ProvDb,
    terms: TermTable,
    budget: usize,
}

impl IncProv {
    /// Full capture of the initial state. Refuses programs with negation
    /// (insertion under negation is not monotone — recapture instead).
    pub fn new(
        core: &CoreProgram,
        certain: &[(String, Tuple)],
        prob: &[(String, Tuple, f64)],
        modes: &HashMap<String, ProvMode>,
    ) -> Result<IncProv, ProvError> {
        let has_negation = core
            .rules
            .iter()
            .any(|r| r.body.iter().any(|l| matches!(l, CoreLiteral::Neg(_))));
        if has_negation {
            return Err(ProvError::Eval(crate::naive::EvalError::Unsupported(
                "incremental provenance over negation (insertion is not monotone); \
                 recapture with run_prov",
            )));
        }
        // @terms values need the checker's shared table; the maintainer keeps
        // its own — refuse rather than mis-unify (recapture covers it).
        let uses_terms = core.rules.iter().any(|r| {
            use strata_ir::core::CoreTerm;
            let compound = |t: &CoreTerm| matches!(t, CoreTerm::Compound { .. });
            r.head.args.iter().any(compound)
                || r.body.iter().any(|l| {
                    let (CoreLiteral::Pos(a) | CoreLiteral::Neg(a)) = l;
                    a.args.iter().any(compound)
                })
        }) || certain
            .iter()
            .map(|(_, t)| t)
            .chain(prob.iter().map(|(_, t, _)| t))
            .any(|t| {
                t.iter()
                    .any(|v| matches!(v, strata_ir::value::GroundVal::Term(_)))
            });
        if uses_terms {
            return Err(ProvError::Eval(crate::naive::EvalError::Unsupported(
                "incremental provenance over `@terms` programs; recapture with run_prov",
            )));
        }
        let mut terms = TermTable::new(0);
        let db = crate::provenance::run_prov(core, certain, prob, modes, &mut terms)?;
        Ok(IncProv {
            core: core.clone(),
            modes: modes.clone(),
            certain: certain.to_vec(),
            prob: prob.to_vec(),
            alive: vec![true; prob.len()],
            db,
            terms,
            budget: MAX_PROOFS_PER_TUPLE,
        })
    }

    /// The maintained database (read-only view).
    pub fn db(&self) -> &ProvDb {
        &self.db
    }

    /// The live soft facts, with their leaf indices (grad/query alignment).
    pub fn soft_facts(&self) -> impl Iterator<Item = (usize, &(String, Tuple, f64))> {
        self.prob.iter().enumerate().filter(|(i, _)| self.alive[*i])
    }

    /// Insert a new soft fact; returns its zero-based leaf index — the value
    /// [`Self::delete_soft`] takes and [`Self::soft_facts`] yields (the proof
    /// literal is `+(index+1)`). Monotone: seed the new leaf's proof and
    /// finish the fixpoint from the current state.
    ///
    /// On `Err` (budget), the maintainer may hold a partially updated state —
    /// treat it as poisoned and rebuild with [`IncProv::new`].
    pub fn insert_soft(&mut self, pred: &str, tuple: Tuple, p: f64) -> Result<usize, ProvError> {
        if !(0.0..=1.0).contains(&p) {
            return Err(ProvError::BadProbability(p));
        }
        if !self.core.predicates.iter().any(|d| d.name == pred) {
            return Err(ProvError::Eval(crate::naive::EvalError::UnknownPred(
                pred.to_string(),
            )));
        }
        self.prob.push((pred.to_string(), tuple.clone(), p));
        self.alive.push(true);
        let leaf = self.prob.len() - 1; // zero-based; literal is +(leaf+1)
        let set = self
            .db
            .rels
            .entry(pred.to_string())
            .or_default()
            .entry(tuple)
            .or_default();
        insert_minimal(set, Proof::from([(leaf + 1) as i64]));
        saturate(
            &mut self.db,
            &self.core,
            &self.prob,
            &self.modes,
            self.budget,
            &mut self.terms,
        )?;
        Ok(leaf)
    }

    /// Delete the soft fact at `leaf` (as returned by [`Self::soft_facts`] /
    /// [`Self::insert_soft`]): drop every proof using it, remove tuples left
    /// with no support, then resume the fixpoint (a no-op for exact capture;
    /// it refills `Prov_k` predicates whose kept proofs died).
    /// On `Err` (budget), treat the maintainer as poisoned (see [`Self::insert_soft`]).
    pub fn delete_soft(&mut self, leaf: usize) -> Result<(), ProvError> {
        if self.alive.get(leaf) != Some(&true) {
            return Err(ProvError::Eval(crate::naive::EvalError::Unsupported(
                "delete of an unknown or already-deleted soft-fact leaf",
            )));
        }
        self.alive[leaf] = false;
        let lit = (leaf + 1) as i64;
        for rel in self.db.rels.values_mut() {
            rel.retain(|_, proofs| {
                proofs.retain(|proof| !proof.contains(&lit) && !proof.contains(&-lit));
                !proofs.is_empty()
            });
        }
        saturate(
            &mut self.db,
            &self.core,
            &self.prob,
            &self.modes,
            self.budget,
            &mut self.terms,
        )
    }

    /// Re-prune every `Prov_k` predicate (used after weight edits, if any).
    #[allow(dead_code)]
    fn reprune(&mut self) {
        for (pred, mode) in &self.modes {
            if let ProvMode::TopK(k) = mode {
                if let Some(rel) = self.db.rels.get_mut(pred) {
                    for proofs in rel.values_mut() {
                        prune_top_k(proofs, *k as usize, &self.prob);
                    }
                }
            }
        }
    }

    /// Returns the leaf index whose insert returns it — for tests.
    pub fn leaf_count(&self) -> usize {
        self.prob.len()
    }

    /// The certain seeds (unchanged by soft edits).
    pub fn certain(&self) -> &[(String, Tuple)] {
        &self.certain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::ProofSet;
    use strata_ir::core::{CoreAtom, CoreLiteral, CorePred, CoreRule, CoreTerm, Semiring};
    use strata_ir::dict::SymbolId;
    use strata_ir::value::GroundVal;

    fn v(slot: u32) -> CoreTerm {
        CoreTerm::Var { slot }
    }
    fn sym(n: u32) -> GroundVal {
        GroundVal::Sym(SymbolId(n))
    }
    fn tc_program() -> CoreProgram {
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

    /// The maintained db must equal a from-scratch recapture of the live facts.
    fn assert_matches_recapture(inc: &IncProv, modes: &HashMap<String, ProvMode>) {
        // Rebuild the live prob list with ORIGINAL leaf indices via padding:
        // recapture uses only live facts, so proofs come back with different
        // indices — compare via re-indexing map.
        let live: Vec<(usize, (String, Tuple, f64))> = inc
            .prob
            .iter()
            .cloned()
            .enumerate()
            .filter(|(i, _)| inc.alive[*i])
            .collect();
        let fresh_prob: Vec<(String, Tuple, f64)> = live.iter().map(|(_, f)| f.clone()).collect();
        let fresh = crate::provenance::run_prov(
            &inc.core,
            &inc.certain,
            &fresh_prob,
            modes,
            &mut TermTable::new(0),
        )
        .expect("recapture");
        // Map fresh leaf index (position in fresh_prob) → original index.
        let remap: Vec<i64> = live.iter().map(|(orig, _)| (*orig + 1) as i64).collect();
        let mut fresh_mapped = std::collections::BTreeMap::new();
        for (pred, rel) in &fresh.rels {
            let mut m = std::collections::BTreeMap::new();
            for (tuple, proofs) in rel {
                let mapped: ProofSet = proofs
                    .iter()
                    .map(|p| {
                        p.iter()
                            .map(|&l| {
                                let idx = (l.abs() - 1) as usize;
                                remap[idx] * l.signum()
                            })
                            .collect()
                    })
                    .collect();
                m.insert(tuple.clone(), mapped);
            }
            fresh_mapped.insert(pred.clone(), m);
        }
        assert_eq!(
            inc.db.rels, fresh_mapped,
            "incremental drifted from recapture"
        );
    }

    fn edge(x: u32, y: u32, p: f64) -> (String, Tuple, f64) {
        ("edge".into(), vec![sym(x), sym(y)], p)
    }

    #[test]
    fn insert_then_delete_round_trips_exact() {
        let core = tc_program();
        let modes = HashMap::new();
        let mut inc = IncProv::new(&core, &[], &[edge(0, 1, 0.5)], &modes).unwrap();
        assert_matches_recapture(&inc, &modes);

        let l1 = inc.insert_soft("edge", vec![sym(1), sym(2)], 0.7).unwrap();
        assert_matches_recapture(&inc, &modes);
        assert!(inc.db.rels["path"].contains_key(&vec![sym(0), sym(2)]));

        let _l2 = inc.insert_soft("edge", vec![sym(0), sym(2)], 0.3).unwrap();
        assert_matches_recapture(&inc, &modes);

        // The round trip the first external caller will write: delete what
        // insert returned. (This line once needed a manual `- 1` — the
        // index-contract defect an external review caught.)
        inc.delete_soft(l1).unwrap(); // delete edge(1,2)
        assert_matches_recapture(&inc, &modes);
        // path(0,2) survives via the direct soft edge; the 2-hop proof is gone.
        let ps = &inc.db.rels["path"][&vec![sym(0), sym(2)]];
        assert_eq!(ps.len(), 1);
    }

    #[test]
    fn topk_refills_after_deletion() {
        let core = tc_program();
        let modes = HashMap::from([("path".to_string(), ProvMode::TopK(1))]);
        // Two routes 0→2: direct (0.9) and 2-hop (0.64); top-1 keeps direct.
        let prob = vec![edge(0, 2, 0.9), edge(0, 1, 0.8), edge(1, 2, 0.8)];
        let mut inc = IncProv::new(&core, &[], &prob, &modes).unwrap();
        let target = vec![sym(0), sym(2)];
        assert_eq!(inc.db.rels["path"][&target].len(), 1);
        // Delete the direct edge: the pruned 2-hop proof must be re-derived.
        inc.delete_soft(0).unwrap();
        let ps = &inc.db.rels["path"][&target];
        assert_eq!(ps.len(), 1, "top-1 refilled from the surviving route");
        assert_eq!(ps.iter().next().unwrap().len(), 2, "the 2-hop proof");
        assert_matches_recapture(&inc, &modes);
    }

    #[test]
    fn negation_is_refused_at_construction() {
        let mut core = tc_program();
        core.predicates.push(CorePred {
            name: "q".into(),
            arity: 2,
            semiring: Semiring::Bool,
            stratum: 1,
        });
        core.rules.push(CoreRule {
            head: CoreAtom {
                pred: "q".into(),
                args: vec![v(0), v(1)],
            },
            body: vec![
                CoreLiteral::Pos(CoreAtom {
                    pred: "edge".into(),
                    args: vec![v(0), v(1)],
                }),
                CoreLiteral::Neg(CoreAtom {
                    pred: "path".into(),
                    args: vec![v(0), v(1)],
                }),
            ],
            stratum: 1,
            var_count: 2,
            neg_weight_cycle_check: false,
        });
        core.num_strata = 2;
        assert!(IncProv::new(&core, &[], &[edge(0, 1, 0.5)], &HashMap::new()).is_err());
    }

    #[test]
    fn wiring_errors_are_typed_not_silent_or_panicking() {
        let core = tc_program();
        let mut inc = IncProv::new(&core, &[], &[edge(0, 1, 0.5)], &HashMap::new()).unwrap();
        // Undeclared predicate: refused, no phantom relation created.
        assert!(inc.insert_soft("nope", vec![sym(0)], 0.5).is_err());
        assert!(!inc.db.rels.contains_key("nope"));
        // Double delete: a typed error, not a panic.
        inc.delete_soft(0).unwrap();
        assert!(inc.delete_soft(0).is_err());
        assert!(inc.delete_soft(99).is_err());
        // @terms programs are refused at construction.
        let mut tcore = tc_program();
        tcore.rules[0].head.args[0] = CoreTerm::Compound {
            functor: SymbolId(7),
            args: vec![v(0)],
        };
        assert!(IncProv::new(&tcore, &[], &[], &HashMap::new()).is_err());
    }

    #[test]
    fn fuzz_edit_scripts_match_recapture() {
        // Random scripts of inserts/deletes over random soft graphs; the
        // maintained db must equal a from-scratch recapture after EVERY step.
        let mut seed = 0xfeed_beef_dead_cafeu64;
        let mut rng = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let core = tc_program();
        for case in 0..25 {
            let modes = if case % 2 == 0 {
                HashMap::new()
            } else {
                HashMap::from([("path".to_string(), ProvMode::TopK(2))])
            };
            let nodes = 3 + rng() % 3;
            let mut inc = IncProv::new(&core, &[], &[], &modes).unwrap();
            let mut live: Vec<usize> = Vec::new();
            for _step in 0..10 {
                if live.is_empty() || rng() % 3 != 0 {
                    let x = (rng() % nodes) as u32;
                    let y = ((x as u64 + 1 + rng() % (nodes - 1)) % nodes) as u32;
                    let p = (rng() % 1001) as f64 / 1000.0;
                    let leaf = inc.insert_soft("edge", vec![sym(x), sym(y)], p).unwrap();
                    live.push(leaf);
                } else {
                    let pick = (rng() % live.len() as u64) as usize;
                    let leaf = live.swap_remove(pick);
                    inc.delete_soft(leaf).unwrap();
                }
                assert_matches_recapture(&inc, &modes);
            }
        }
    }
}
