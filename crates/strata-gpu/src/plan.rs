//! Query planning for worst-case-optimal joins. [Phase 2: CBO + hypertree decomposition]
//!
//! The WCOJ kernels (see [`crate::count_triangles`]) execute a *plan*; this
//! module produces it. Two classic pieces, both pure CPU logic (no GPU):
//!
//! - **Hypertree decomposition** (GYO ear-removal): tests whether a conjunctive
//!   query is α-acyclic and, if so, yields a join tree (each atom a width-1 bag,
//!   Yannakakis-evaluable). What GYO can't peel off is the cyclic core — a single
//!   bag the WCOJ handles (width = its atom count; 3 for a triangle).
//! - **Cost-based optimizer** (Selinger-style): choose the WCOJ variable order
//!   that minimizes an estimated cost, so a plan leads with the most selective
//!   relations instead of a fixed order.
//!
//! Together they turn an arbitrary body — not just the hand-written triangle/K4
//! kernels — into a WCOJ execution plan.

use std::collections::{BTreeSet, HashMap};

/// A logic variable (query attribute).
pub type Var = u32;

/// One body atom: a relation applied to a list of variables, e.g. `edge(a, b)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Atom {
    pub rel: String,
    pub vars: Vec<Var>,
}

impl Atom {
    pub fn new(rel: &str, vars: &[Var]) -> Self {
        Atom {
            rel: rel.to_string(),
            vars: vars.to_vec(),
        }
    }
    fn varset(&self) -> BTreeSet<Var> {
        self.vars.iter().copied().collect()
    }
}

/// A conjunctive query body (the atoms to join).
#[derive(Clone, Debug)]
pub struct Query {
    pub atoms: Vec<Atom>,
}

impl Query {
    pub fn new(atoms: Vec<Atom>) -> Self {
        Query { atoms }
    }
    /// All distinct variables in the body, ascending.
    pub fn vars(&self) -> Vec<Var> {
        let s: BTreeSet<Var> = self
            .atoms
            .iter()
            .flat_map(|a| a.vars.iter().copied())
            .collect();
        s.into_iter().collect()
    }
}

// --------------------------------------------------------------------------
// Hypertree decomposition (GYO)
// --------------------------------------------------------------------------

/// The result of decomposing a query's hypergraph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decomposition {
    /// True iff the query is α-acyclic (GYO reduces it fully).
    pub acyclic: bool,
    /// Max bag size (in atoms). 1 iff acyclic; else the cyclic-core size.
    pub width: usize,
    /// Atom indices GYO could not peel off — a single WCOJ bag (empty iff acyclic).
    pub cyclic_core: Vec<usize>,
    /// Ear-removal order as `(atom, parent atom)` — the join tree for the peeled
    /// (acyclic) part. `parent = None` for a root / the last atom removed.
    pub join_tree: Vec<(usize, Option<usize>)>,
}

/// Decompose `q` by GYO ear-removal. An atom is an *ear* when the variables it
/// shares with the rest are all contained in one other atom (its parent), or it
/// shares nothing. Peel ears until none remain; the residue is the cyclic core.
pub fn hypertree_decompose(q: &Query) -> Decomposition {
    let n = q.atoms.len();
    let vs: Vec<BTreeSet<Var>> = q.atoms.iter().map(|a| a.varset()).collect();
    let mut alive: Vec<usize> = (0..n).collect();
    let mut join_tree = Vec::new();

    loop {
        let mut ear: Option<(usize, Option<usize>)> = None;
        for &e in &alive {
            // Variables of e that also occur in some other alive atom.
            let shared: BTreeSet<Var> = vs[e]
                .iter()
                .copied()
                .filter(|v| alive.iter().any(|&o| o != e && vs[o].contains(v)))
                .collect();
            // A witness atom containing all shared variables → the parent.
            let witness = alive
                .iter()
                .copied()
                .find(|&o| o != e && shared.iter().all(|v| vs[o].contains(v)));
            if shared.is_empty() || witness.is_some() {
                ear = Some((e, witness));
                break;
            }
        }
        match ear {
            Some((e, parent)) => {
                join_tree.push((e, parent));
                alive.retain(|&x| x != e);
            }
            None => break,
        }
    }

    let acyclic = alive.is_empty();
    Decomposition {
        acyclic,
        width: if acyclic { 1 } else { alive.len() },
        cyclic_core: alive,
        join_tree,
    }
}

// --------------------------------------------------------------------------
// Cost-based optimizer (Selinger-style variable ordering)
// --------------------------------------------------------------------------

/// Statistics for cost estimation: relation cardinalities and per-variable
/// domain (distinct-value) sizes.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub card: HashMap<String, u64>,
    pub domain: HashMap<Var, u64>,
}

impl Stats {
    fn card_of(&self, rel: &str) -> u128 {
        *self.card.get(rel).unwrap_or(&1000) as u128
    }
    fn domain_of(&self, v: Var) -> u128 {
        (*self.domain.get(&v).unwrap_or(&1)).max(1) as u128
    }
}

/// Textbook independence estimate of the join size of a set of atoms:
/// `∏ |R| / ∏_v D(v)^{(k_v − 1)}` where `k_v` is how many of the atoms contain v.
fn est_join_size(atoms: &[&Atom], stats: &Stats) -> u128 {
    let mut num: u128 = 1;
    let mut count: HashMap<Var, u32> = HashMap::new();
    for a in atoms {
        num = num.saturating_mul(stats.card_of(&a.rel));
        for &v in &a.vars {
            *count.entry(v).or_insert(0) += 1;
        }
    }
    let mut den: u128 = 1;
    for (v, k) in count {
        if k >= 2 {
            den = den.saturating_mul(stats.domain_of(v).saturating_pow(k - 1));
        }
    }
    (num / den.max(1)).max(1)
}

/// Estimated number of distinct bindings of the variables in `prefix`: the join
/// of the atoms it fully covers, times the domain of any prefix variable not yet
/// covered by a closed atom (those range freely — this is what penalizes binding
/// two unrelated variables before their joining variable).
fn est_prefix(prefix: &BTreeSet<Var>, q: &Query, stats: &Stats) -> u128 {
    let closed: Vec<&Atom> = q
        .atoms
        .iter()
        .filter(|a| a.vars.iter().all(|v| prefix.contains(v)))
        .collect();
    let mut est = est_join_size(&closed, stats);
    let covered: BTreeSet<Var> = closed.iter().flat_map(|a| a.vars.iter().copied()).collect();
    for &v in prefix {
        if !covered.contains(&v) {
            est = est.saturating_mul(stats.domain_of(v));
        }
    }
    est
}

/// Cost of a variable order: the sum of the estimated partial-result size after
/// each variable is bound — the classic Selinger "sum of intermediate sizes",
/// specialized to a WCOJ's attribute-at-a-time evaluation.
fn order_cost(order: &[Var], q: &Query, stats: &Stats) -> u128 {
    let mut prefix: BTreeSet<Var> = BTreeSet::new();
    let mut cost: u128 = 0;
    for &v in order {
        prefix.insert(v);
        cost = cost.saturating_add(est_prefix(&prefix, q, stats));
    }
    cost
}

fn permutations(items: &[Var]) -> Vec<Vec<Var>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut out = Vec::new();
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let x = rest.remove(i);
        for mut p in permutations(&rest) {
            p.insert(0, x);
            out.push(p);
        }
    }
    out
}

/// Choose the WCOJ variable order minimizing [`order_cost`]. Enumerates all
/// orders for ≤ 8 variables (exact); above that, a greedy min-cost extension.
pub fn cost_based_order(q: &Query, stats: &Stats) -> (Vec<Var>, u128) {
    let vars = q.vars();
    if vars.len() <= 8 {
        return permutations(&vars)
            .into_iter()
            .map(|o| {
                let c = order_cost(&o, q, stats);
                (o, c)
            })
            .min_by_key(|(_, c)| *c)
            .unwrap();
    }
    // Greedy: repeatedly append the variable giving the least incremental cost.
    let mut order: Vec<Var> = Vec::new();
    let mut remaining: BTreeSet<Var> = vars.into_iter().collect();
    while !remaining.is_empty() {
        let next = *remaining
            .iter()
            .min_by_key(|&&v| {
                let mut cand = order.clone();
                cand.push(v);
                order_cost(&cand, q, stats)
            })
            .unwrap();
        order.push(next);
        remaining.remove(&next);
    }
    let c = order_cost(&order, q, stats);
    (order, c)
}

/// A full WCOJ plan: how the body decomposes, and the variable order to run.
#[derive(Clone, Debug)]
pub struct Plan {
    pub decomposition: Decomposition,
    pub order: Vec<Var>,
    pub est_cost: u128,
    /// Tensor-contraction width for the same body (Phase 6): the induced
    /// treewidth + 1 of the min-degree elimination order. A second, independent
    /// read on the query's intrinsic hardness alongside the hypertree width — for
    /// an acyclic body both are 1; for a cyclic core (e.g. the triangle) both
    /// report the same bag size.
    pub contraction_width: usize,
}

/// Plan a query: decompose it, pick a cost-based variable order, and record the
/// tensor-contraction width.
pub fn plan(q: &Query, stats: &Stats) -> Plan {
    let decomposition = hypertree_decompose(q);
    let (order, est_cost) = cost_based_order(q, stats);
    let contraction_width = crate::contraction::contraction_order(q).width;
    Plan {
        decomposition,
        order,
        est_cost,
        contraction_width,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(a: Var, b: Var) -> Atom {
        Atom::new("edge", &[a, b])
    }

    #[test]
    fn triangle_is_cyclic_core() {
        // edge(a,b), edge(b,c), edge(a,c) — the 3-cycle: GYO peels nothing.
        let q = Query::new(vec![e(0, 1), e(1, 2), e(0, 2)]);
        let d = hypertree_decompose(&q);
        assert!(!d.acyclic);
        assert_eq!(d.width, 3);
        assert_eq!(d.cyclic_core.len(), 3);
    }

    #[test]
    fn path_is_acyclic() {
        // edge(a,b), edge(b,c), edge(c,d) — a path is α-acyclic (width 1).
        let q = Query::new(vec![e(0, 1), e(1, 2), e(2, 3)]);
        let d = hypertree_decompose(&q);
        assert!(d.acyclic);
        assert_eq!(d.width, 1);
        assert!(d.cyclic_core.is_empty());
        assert_eq!(d.join_tree.len(), 3);
    }

    #[test]
    fn star_is_acyclic() {
        let q = Query::new(vec![e(0, 1), e(0, 2), e(0, 3)]);
        assert!(hypertree_decompose(&q).acyclic);
    }

    #[test]
    fn four_cycle_is_cyclic() {
        let q = Query::new(vec![e(0, 1), e(1, 2), e(2, 3), e(3, 0)]);
        let d = hypertree_decompose(&q);
        assert!(!d.acyclic);
        assert_eq!(d.width, 4);
    }

    #[test]
    fn cbo_leads_with_the_small_relation() {
        // R(a,b) small, S(b,c) large, shared b. The cheap order binds a,b (opens
        // the small R) before c; the reverse opens the large S first.
        let q = Query::new(vec![Atom::new("R", &[0, 1]), Atom::new("S", &[1, 2])]);
        let mut stats = Stats::default();
        stats.card.insert("R".into(), 10);
        stats.card.insert("S".into(), 1000);
        stats.domain.insert(0, 10); // domain of a
        stats.domain.insert(1, 100); // shared var b
        stats.domain.insert(2, 1000); // domain of c

        let good = order_cost(&[0, 1, 2], &q, &stats); // a,b,c → opens small R first
        let bad = order_cost(&[2, 1, 0], &q, &stats); // c,b,a → opens large S first
        assert!(good < bad, "good={good} bad={bad}");

        let (order, cost) = cost_based_order(&q, &stats);
        assert_eq!(cost, good);
        // the chosen order opens R (vars 0,1) before variable 2
        let p0 = order.iter().position(|&v| v == 0).unwrap();
        let p2 = order.iter().position(|&v| v == 2).unwrap();
        assert!(
            p0 < p2,
            "order {order:?} should bind R's vars before S's tail"
        );
    }

    #[test]
    fn per_superstep_reoptimization() {
        // The same body, re-planned with the cardinalities of two different
        // fixpoint supersteps: as the smaller relation flips, the cost-based
        // order flips with it. This is per-superstep re-optimization.
        let q = Query::new(vec![Atom::new("R", &[0, 1]), Atom::new("S", &[1, 2])]);
        let mut s1 = Stats::default();
        s1.card.insert("R".into(), 10);
        s1.card.insert("S".into(), 1000);
        for (v, d) in [(0, 10), (1, 100), (2, 1000)] {
            s1.domain.insert(v, d);
        }
        // Superstep 2: R has grown large, S is now the small one.
        let mut s2 = Stats::default();
        s2.card.insert("R".into(), 1000);
        s2.card.insert("S".into(), 10);
        for (v, d) in [(0, 1000), (1, 100), (2, 10)] {
            s2.domain.insert(v, d);
        }
        let (o1, _) = cost_based_order(&q, &s1);
        let (o2, _) = cost_based_order(&q, &s2);
        // Round 1 leads with R's exclusive var (0); round 2 leads with S's (2).
        assert_eq!(o1.first(), Some(&0), "round 1 order {o1:?}");
        assert_eq!(o2.first(), Some(&2), "round 2 order {o2:?}");
        assert_ne!(o1, o2, "re-optimization should adapt the plan");
    }

    #[test]
    fn plan_triangle() {
        let q = Query::new(vec![e(0, 1), e(1, 2), e(0, 2)]);
        let mut stats = Stats::default();
        stats.card.insert("edge".into(), 1000);
        stats.domain.insert(0, 100);
        stats.domain.insert(1, 100);
        stats.domain.insert(2, 100);
        let p = plan(&q, &stats);
        assert!(!p.decomposition.acyclic); // needs WCOJ
        assert_eq!(p.order.len(), 3);
        // Phase-6 tensor route agrees: triangle's cyclic-core bag = contraction
        // width = 3 (treewidth 2).
        assert_eq!(p.contraction_width, 3);
        assert_eq!(p.contraction_width, p.decomposition.width);
    }

    #[test]
    fn plan_reports_contraction_width_for_acyclic() {
        // a path is acyclic (hypertree width 1); its contraction width is 2
        // (treewidth 1 + 1) — the two measures read the same easy body two ways.
        let q = Query::new(vec![e(0, 1), e(1, 2), e(2, 3)]);
        let p = plan(&q, &Stats::default());
        assert!(p.decomposition.acyclic);
        assert_eq!(p.contraction_width, 2);
    }
}
