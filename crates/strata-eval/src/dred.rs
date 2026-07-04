//! DRed — Delete/Rederive incremental evaluation. [Phase 6, spec §6]
//!
//! Spec §6: recomputing the whole fixpoint on every knowledge-graph update
//! defeats the point of acceleration; v1 does **DRed** — overdeletion cascades
//! the consequences of a removed fact, then rederivation restores those that
//! still have an alternative derivation. This is the CPU reference for that
//! algorithm (Gupta–Mumick–Subrahmanian), checked against a from-scratch
//! recompute after every update.
//!
//! A self-contained positive-Datalog engine: EDB facts + rules, least-fixpoint
//! evaluation. EDB and IDB predicates are disjoint (the standard DRed setting),
//! so EDB facts are never rederived — only IDB consequences move.

use std::collections::{HashMap, HashSet};

/// A ground symbol.
pub type Sym = i64;
/// A ground atom: predicate + arguments.
pub type Fact = (String, Vec<Sym>);

/// A term in a rule: a variable (by id) or a constant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    Var(u32),
    Const(Sym),
}

/// A rule `head :- body`.
#[derive(Clone, Debug)]
pub struct Rule {
    pub head: (String, Vec<Term>),
    pub body: Vec<(String, Vec<Term>)>,
}

/// All derivations valid over `facts`: each `(head_fact, body_facts)` is one rule
/// grounding whose whole body is present. Used both for the least fixpoint and
/// for the DRed cascade (which keys on whether a body fact was deleted).
fn derivations(rules: &[Rule], facts: &HashSet<Fact>) -> Vec<(Fact, Vec<Fact>)> {
    let mut by_pred: HashMap<&str, Vec<&Fact>> = HashMap::new();
    for f in facts {
        by_pred.entry(f.0.as_str()).or_default().push(f);
    }
    let mut out = Vec::new();
    for r in rules {
        let mut binding: HashMap<u32, Sym> = HashMap::new();
        join(r, 0, &by_pred, &mut binding, &mut Vec::new(), &mut out);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn join(
    r: &Rule,
    i: usize,
    by_pred: &HashMap<&str, Vec<&Fact>>,
    binding: &mut HashMap<u32, Sym>,
    body_facts: &mut Vec<Fact>,
    out: &mut Vec<(Fact, Vec<Fact>)>,
) {
    if i == r.body.len() {
        // instantiate the head
        let args: Vec<Sym> = r
            .head
            .1
            .iter()
            .map(|t| match t {
                Term::Const(c) => *c,
                Term::Var(v) => binding[v],
            })
            .collect();
        out.push(((r.head.0.clone(), args), body_facts.clone()));
        return;
    }
    let (pred, pat) = &r.body[i];
    let empty = Vec::new();
    for f in by_pred.get(pred.as_str()).unwrap_or(&empty) {
        if f.1.len() != pat.len() {
            continue;
        }
        // try to unify pattern with the fact's args, tracking new bindings.
        let mut fresh: Vec<u32> = Vec::new();
        let mut ok = true;
        for (t, &val) in pat.iter().zip(&f.1) {
            match t {
                Term::Const(c) => {
                    if *c != val {
                        ok = false;
                        break;
                    }
                }
                Term::Var(v) => match binding.get(v) {
                    Some(&b) => {
                        if b != val {
                            ok = false;
                            break;
                        }
                    }
                    None => {
                        binding.insert(*v, val);
                        fresh.push(*v);
                    }
                },
            }
        }
        if ok {
            body_facts.push((*f).clone());
            join(r, i + 1, by_pred, binding, body_facts, out);
            body_facts.pop();
        }
        for v in fresh {
            binding.remove(&v);
        }
    }
}

/// Least fixpoint: EDB plus everything derivable. The from-scratch oracle.
pub fn eval(edb: &HashSet<Fact>, rules: &[Rule]) -> HashSet<Fact> {
    let mut model = edb.clone();
    loop {
        let derivs = derivations(rules, &model);
        let mut changed = false;
        for (h, _) in derivs {
            if model.insert(h) {
                changed = true;
            }
        }
        if !changed {
            return model;
        }
    }
}

/// An incrementally-maintained materialization.
pub struct Dred {
    pub edb: HashSet<Fact>,
    pub rules: Vec<Rule>,
    pub model: HashSet<Fact>,
}

impl Dred {
    pub fn new(edb: HashSet<Fact>, rules: Vec<Rule>) -> Self {
        let model = eval(&edb, &rules);
        Dred { edb, rules, model }
    }

    /// Insert EDB facts and semi-naively extend the model.
    pub fn insert(&mut self, add: &[Fact]) {
        for f in add {
            self.edb.insert(f.clone());
            self.model.insert(f.clone());
        }
        // re-saturate (bounded programs converge quickly).
        loop {
            let derivs = derivations(&self.rules, &self.model);
            let mut changed = false;
            for (h, _) in derivs {
                if self.model.insert(h) {
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Delete EDB facts and repair the model by DRed: overdelete the cascade,
    /// then rederive whatever still has support.
    pub fn delete(&mut self, del: &[Fact]) {
        let del_set: HashSet<Fact> = del.iter().cloned().collect();
        for f in del {
            self.edb.remove(f);
        }

        // Derivations that held before the update (over the old model).
        let derivs = derivations(&self.rules, &self.model);

        // --- overdeletion: any head whose derivation used a deleted/overdeleted
        // body fact is a deletion candidate; cascade to a fixpoint.
        let mut overdel: HashSet<Fact> = del_set.clone();
        loop {
            let mut changed = false;
            for (h, body) in &derivs {
                if !overdel.contains(h) && body.iter().any(|b| overdel.contains(b)) {
                    overdel.insert(h.clone());
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // Surviving facts: model minus the overdeleted set (but never drop a
        // still-present EDB fact — EDB and IDB predicates are disjoint here).
        let mut survived: HashSet<Fact> = self
            .model
            .iter()
            .filter(|f| !overdel.contains(*f) || self.edb.contains(*f))
            .cloned()
            .collect();
        for f in &self.edb {
            survived.insert(f.clone());
        }

        // --- rederivation: re-saturate from the survivors; anything with a
        // surviving alternative derivation comes back.
        loop {
            let ds = derivations(&self.rules, &survived);
            let mut changed = false;
            for (h, _) in ds {
                if survived.insert(h) {
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        self.model = survived;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(pred: &str, args: &[Sym]) -> Fact {
        (pred.to_string(), args.to_vec())
    }

    /// Transitive closure: path(X,Y):-edge(X,Y). path(X,Z):-edge(X,Y),path(Y,Z).
    fn tc_rules() -> Vec<Rule> {
        vec![
            Rule {
                head: ("path".into(), vec![Term::Var(0), Term::Var(1)]),
                body: vec![("edge".into(), vec![Term::Var(0), Term::Var(1)])],
            },
            Rule {
                head: ("path".into(), vec![Term::Var(0), Term::Var(2)]),
                body: vec![
                    ("edge".into(), vec![Term::Var(0), Term::Var(1)]),
                    ("path".into(), vec![Term::Var(1), Term::Var(2)]),
                ],
            },
        ]
    }

    fn edges(es: &[(i64, i64)]) -> HashSet<Fact> {
        es.iter().map(|&(a, b)| f("edge", &[a, b])).collect()
    }

    #[test]
    fn delete_matches_recompute_chain() {
        // chain 0->1->2->3; delete the middle edge 1->2.
        let mut d = Dred::new(edges(&[(0, 1), (1, 2), (2, 3)]), tc_rules());
        d.delete(&[f("edge", &[1, 2])]);
        let fresh = eval(&edges(&[(0, 1), (2, 3)]), &tc_rules());
        assert_eq!(d.model, fresh, "DRed model diverged from recompute");
        // path(0,3) must be gone; path(0,1) and path(2,3) remain.
        assert!(!d.model.contains(&f("path", &[0, 3])));
        assert!(d.model.contains(&f("path", &[0, 1])));
        assert!(d.model.contains(&f("path", &[2, 3])));
    }

    #[test]
    fn delete_keeps_alternatively_supported_fact() {
        // 0->1, 1->2, 0->2 (a triangle-ish): path(0,2) has two derivations.
        // deleting 0->2 must NOT remove path(0,2) (still via 0->1->2).
        let mut d = Dred::new(edges(&[(0, 1), (1, 2), (0, 2)]), tc_rules());
        assert!(d.model.contains(&f("path", &[0, 2])));
        d.delete(&[f("edge", &[0, 2])]);
        assert!(
            d.model.contains(&f("path", &[0, 2])),
            "rederivation should keep path(0,2) via 0->1->2"
        );
        assert_eq!(d.model, eval(&edges(&[(0, 1), (1, 2)]), &tc_rules()));
    }

    #[test]
    fn fuzz_dred_equals_recompute() {
        // random graphs, random deletion sets; DRed == recompute every time.
        let mut seed = 0xDEAD_BEEFu64;
        let mut nxt = |m: i64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as i64 % m
        };
        for _ in 0..60 {
            let n = 6;
            // unique edge set (facts dedup), then split into keep / delete.
            let mut eset: std::collections::BTreeSet<(i64, i64)> =
                std::collections::BTreeSet::new();
            for _ in 0..10 {
                eset.insert((nxt(n), nxt(n)));
            }
            let mut d = Dred::new(edges(&eset.iter().copied().collect::<Vec<_>>()), tc_rules());
            let mut del = Vec::new();
            let mut remaining: Vec<(i64, i64)> = Vec::new();
            for &(a, b) in &eset {
                if nxt(2) == 0 {
                    del.push(f("edge", &[a, b]));
                } else {
                    remaining.push((a, b));
                }
            }
            d.delete(&del);
            let fresh = eval(&edges(&remaining), &tc_rules());
            assert_eq!(
                d.model, fresh,
                "DRed != recompute for edges {eset:?} del {del:?}"
            );
        }
    }

    #[test]
    fn insert_then_delete_roundtrip() {
        let mut d = Dred::new(edges(&[(0, 1)]), tc_rules());
        d.insert(&[f("edge", &[1, 2])]);
        assert!(d.model.contains(&f("path", &[0, 2])));
        d.delete(&[f("edge", &[1, 2])]);
        assert_eq!(d.model, eval(&edges(&[(0, 1)]), &tc_rules()));
    }
}
