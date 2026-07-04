//! Tensor-contraction query plan. [Phase 6, spec §9 "tensor-contraction plan"]
//!
//! A conjunctive query is a tensor network: each atom `r(x,y,…)` is an indicator
//! tensor over its variables (`1` where the tuple is present), and a shared
//! variable is a contracted index. Fully contracting the network — sum-product
//! variable elimination — counts the query's satisfying assignments, i.e. the
//! join size. The *plan* is the elimination order; its quality is the width of
//! the largest intermediate tensor, which equals the induced treewidth + 1 and
//! bounds the work (the tensor analogue of the AGM/hypertree bound in
//! [`crate::plan`]).
//!
//! This module is pure CPU: it produces the order (a min-degree elimination
//! heuristic) and also *executes* the contraction as a reference, so the tensor
//! route can be checked bit-for-bit against a brute-force join.

use crate::plan::{Query, Var};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A contraction plan: the variable elimination order and the induced width
/// (max intermediate tensor arity = treewidth + 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contraction {
    pub order: Vec<Var>,
    pub width: usize,
}

/// The interaction graph: an edge between two variables that co-occur in an atom.
fn interaction(q: &Query) -> HashMap<Var, BTreeSet<Var>> {
    let mut g: HashMap<Var, BTreeSet<Var>> = HashMap::new();
    for v in q.vars() {
        g.entry(v).or_default();
    }
    for a in &q.atoms {
        for (i, &u) in a.vars.iter().enumerate() {
            for &w in &a.vars[i + 1..] {
                if u != w {
                    g.get_mut(&u).unwrap().insert(w);
                    g.get_mut(&w).unwrap().insert(u);
                }
            }
        }
    }
    g
}

/// Min-degree elimination ordering: repeatedly eliminate the lowest-degree
/// variable, connecting its neighbours (fill-in). The width is the largest
/// closed neighbourhood eliminated (= treewidth + 1 for this heuristic).
pub fn contraction_order(q: &Query) -> Contraction {
    let mut g = interaction(q);
    let mut order = Vec::new();
    let mut width = 0usize;
    while !g.is_empty() {
        // pick the min-degree vertex (ties: smallest id, deterministic).
        let v = *g
            .iter()
            .min_by_key(|(id, nbrs)| (nbrs.len(), **id))
            .unwrap()
            .0;
        let nbrs: Vec<Var> = g[&v].iter().copied().collect();
        width = width.max(nbrs.len() + 1); // the bag = v plus its neighbours
                                           // fill-in: make the neighbourhood a clique.
        for i in 0..nbrs.len() {
            for j in i + 1..nbrs.len() {
                g.get_mut(&nbrs[i]).unwrap().insert(nbrs[j]);
                g.get_mut(&nbrs[j]).unwrap().insert(nbrs[i]);
            }
        }
        for n in &nbrs {
            g.get_mut(n).unwrap().remove(&v);
        }
        g.remove(&v);
        order.push(v);
    }
    Contraction { order, width }
}

// --- reference contraction (sum-product variable elimination) ----------------

/// A factor: a map from an assignment of its `vars` (in `vars` order) to a
/// nonnegative count. Atoms start as indicator factors (count 1 per tuple).
#[derive(Clone, Debug)]
struct Factor {
    vars: Vec<Var>,
    table: BTreeMap<Vec<i64>, u64>,
}

impl Factor {
    fn from_atom(vars: &[Var], tuples: &[Vec<i64>]) -> Factor {
        let mut table = BTreeMap::new();
        for t in tuples {
            assert_eq!(t.len(), vars.len(), "arity mismatch");
            *table.entry(t.clone()).or_insert(0) += 1;
        }
        Factor {
            vars: vars.to_vec(),
            table,
        }
    }
    fn val(&self, assign: &HashMap<Var, i64>) -> u64 {
        let key: Vec<i64> = self.vars.iter().map(|v| assign[v]).collect();
        *self.table.get(&key).unwrap_or(&0)
    }
}

/// Multiply the factors mentioning `v`, then sum `v` out — one elimination step.
fn eliminate(factors: Vec<Factor>, v: Var) -> Vec<Factor> {
    let (with, without): (Vec<Factor>, Vec<Factor>) =
        factors.into_iter().partition(|f| f.vars.contains(&v));
    if with.is_empty() {
        return without;
    }
    // union of variables of the factors touching v.
    let mut newvars: Vec<Var> = {
        let s: BTreeSet<Var> = with.iter().flat_map(|f| f.vars.iter().copied()).collect();
        s.into_iter().collect()
    };
    // domain of v = the values it takes across those factors.
    let dom_v: BTreeSet<i64> = with
        .iter()
        .flat_map(|f| {
            let idx = f.vars.iter().position(|&x| x == v).unwrap();
            f.table.keys().map(move |k| k[idx])
        })
        .collect();
    // out vars = newvars minus v.
    newvars.retain(|&x| x != v);

    // enumerate assignments to newvars from the joint support, summing over v.
    // build support of (newvars) by cross-referencing factor tables is costly;
    // instead enumerate the product of per-variable domains restricted to support.
    let domains: HashMap<Var, Vec<i64>> = {
        let mut d: HashMap<Var, BTreeSet<i64>> = HashMap::new();
        for f in &with {
            for (i, &var) in f.vars.iter().enumerate() {
                for k in f.table.keys() {
                    d.entry(var).or_default().insert(k[i]);
                }
            }
        }
        d.into_iter()
            .map(|(k, s)| (k, s.into_iter().collect()))
            .collect()
    };

    let mut table: BTreeMap<Vec<i64>, u64> = BTreeMap::new();
    let mut assign: HashMap<Var, i64> = HashMap::new();
    enumerate(&newvars, 0, &domains, &mut assign, &mut |assign| {
        let mut sum = 0u64;
        for &vv in &dom_v {
            assign.insert(v, vv);
            let prod: u64 = with.iter().map(|f| f.val(assign)).product();
            sum += prod;
        }
        assign.remove(&v);
        if sum > 0 {
            let key: Vec<i64> = newvars.iter().map(|x| assign[x]).collect();
            *table.entry(key).or_insert(0) += sum;
        }
    });

    let mut out = without;
    out.push(Factor {
        vars: newvars,
        table,
    });
    out
}

fn enumerate(
    vars: &[Var],
    i: usize,
    domains: &HashMap<Var, Vec<i64>>,
    assign: &mut HashMap<Var, i64>,
    emit: &mut impl FnMut(&mut HashMap<Var, i64>),
) {
    if i == vars.len() {
        emit(assign);
        return;
    }
    let v = vars[i];
    let empty = Vec::new();
    for &val in domains.get(&v).unwrap_or(&empty) {
        assign.insert(v, val);
        enumerate(vars, i + 1, domains, assign, emit);
    }
    assign.remove(&v);
}

/// Count the query's satisfying assignments by tensor contraction, following the
/// min-degree elimination order. `relations[i]` are the tuples of `q.atoms[i]`.
/// Returns the join size — equal to a brute-force nested-loop join count.
pub fn contract_count(q: &Query, relations: &[Vec<Vec<i64>>]) -> u64 {
    assert_eq!(q.atoms.len(), relations.len());
    let mut factors: Vec<Factor> = q
        .atoms
        .iter()
        .zip(relations)
        .map(|(a, tuples)| Factor::from_atom(&a.vars, tuples))
        .collect();
    for v in contraction_order(q).order {
        factors = eliminate(factors, v);
    }
    // remaining factors are scalars (no vars); multiply.
    factors
        .iter()
        .map(|f| f.table.values().copied().sum::<u64>())
        .product()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Atom;

    /// Brute-force join size: enumerate all assignments over the active domain.
    fn brute(q: &Query, relations: &[Vec<Vec<i64>>]) -> u64 {
        let vars = q.vars();
        let dom: Vec<i64> = {
            let s: BTreeSet<i64> = relations.iter().flatten().flatten().copied().collect();
            s.into_iter().collect()
        };
        let mut count = 0u64;
        let mut assign: HashMap<Var, i64> = HashMap::new();
        fn rec(
            vars: &[Var],
            i: usize,
            dom: &[i64],
            assign: &mut HashMap<Var, i64>,
            q: &Query,
            rels: &[Vec<Vec<i64>>],
            count: &mut u64,
        ) {
            if i == vars.len() {
                let ok = q.atoms.iter().zip(rels).all(|(a, tuples)| {
                    let t: Vec<i64> = a.vars.iter().map(|v| assign[v]).collect();
                    tuples.contains(&t)
                });
                if ok {
                    *count += 1;
                }
                return;
            }
            for &d in dom {
                assign.insert(vars[i], d);
                rec(vars, i + 1, dom, assign, q, rels, count);
            }
        }
        rec(&vars, 0, &dom, &mut assign, q, relations, &mut count);
        count
    }

    fn q_triangle() -> Query {
        // R(a,b), S(b,c), T(a,c)
        Query::new(vec![
            Atom::new("R", &[0, 1]),
            Atom::new("S", &[1, 2]),
            Atom::new("T", &[0, 2]),
        ])
    }

    #[test]
    fn triangle_width_is_two() {
        let c = contraction_order(&q_triangle());
        assert_eq!(c.width, 3, "triangle bag = 3 vars (treewidth 2)");
        assert_eq!(c.order.len(), 3);
    }

    #[test]
    fn path_width_is_one() {
        // a-b, b-c, c-d (a path/chain): treewidth 1, width 2.
        let q = Query::new(vec![
            Atom::new("E", &[0, 1]),
            Atom::new("E", &[1, 2]),
            Atom::new("E", &[2, 3]),
        ]);
        assert_eq!(contraction_order(&q).width, 2);
    }

    #[test]
    fn contraction_counts_triangles() {
        // directed triangle over a small graph; edges reused for R,S,T.
        let edges = vec![vec![0, 1], vec![1, 2], vec![0, 2], vec![2, 0], vec![1, 0]];
        let q = q_triangle();
        let rels = vec![edges.clone(), edges.clone(), edges.clone()];
        assert_eq!(contract_count(&q, &rels), brute(&q, &rels));
    }

    #[test]
    fn contraction_matches_brute_random() {
        // pseudo-random small relations; contraction == brute force on 4-cycle.
        let q = Query::new(vec![
            Atom::new("A", &[0, 1]),
            Atom::new("B", &[1, 2]),
            Atom::new("C", &[2, 3]),
            Atom::new("D", &[3, 0]),
        ]);
        let mut seed = 0x1234_5678u64;
        let mut nxt = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as i64 % 4
        };
        for _ in 0..40 {
            let rel = |n: usize, f: &mut dyn FnMut() -> i64| -> Vec<Vec<i64>> {
                let mut s: BTreeSet<Vec<i64>> = BTreeSet::new();
                for _ in 0..n {
                    s.insert(vec![f(), f()]);
                }
                s.into_iter().collect()
            };
            let rels = vec![
                rel(6, &mut nxt),
                rel(6, &mut nxt),
                rel(6, &mut nxt),
                rel(6, &mut nxt),
            ];
            assert_eq!(
                contract_count(&q, &rels),
                brute(&q, &rels),
                "tensor contraction != brute join"
            );
        }
    }
}
