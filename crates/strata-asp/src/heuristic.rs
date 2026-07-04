//! Search-guidance oracle + the ablation it is judged by. [Phase 6, spec §5.4, §10]
//!
//! Spec §5.4: an oracle proposes soft search guidance (variable priorities /
//! phases — a warm VSIDS init) once per restart; it is **strictly optional**
//! (invariant И3) — the solver runs on its own heuristics without it. Spec §10:
//! the oracle's usefulness is a *hypothesis*, and **ablation is the criterion**.
//!
//! Two oracles are provided, emitted to clingo as `#heuristic` level hints
//! (`--heuristic=Domain`):
//! - [`Oracle`] — a linear model over a single structural feature (degree); the
//!   minimal baseline.
//! - [`Gnn`] — a small **message-passing graph network** over the program's
//!   constraint graph: node features diffuse to neighbours for a few rounds
//!   (GCN-style mean aggregation), so a node's score reflects its whole
//!   neighbourhood, not just its degree. Weights are set offline; a real trained
//!   deep net is engineering breadth, but this is a genuine GNN computation.
//!
//! The [`ablation`](../../tests/ablation.rs) harness fits/uses them and measures
//! the change in clingo's *choices* on held-out instances — the honest test of
//! whether guidance helps (§10). It also runs a **negative control** (an
//! anti-oracle that decides the *least* constrained node first) to show the
//! ablation actually discriminates: a good order helps, a bad order hurts.
//!
//! The oracle never changes which models exist (a level hint only reorders the
//! search), so correctness is invariant — the ablation asserts identical answer
//! sets with and without it.

/// Cheap structural features of an atom in the program's constraint graph.
#[derive(Clone, Copy, Debug, Default)]
pub struct AtomFeatures {
    /// Degree of the atom's object in the constraint graph (how constrained it
    /// is) — e.g. a graph-colouring vertex's degree.
    pub degree: u32,
}

/// A linear oracle: `level = weight · degree`. Higher level ⇒ decided earlier,
/// i.e. "branch on the most-constrained object first" when `weight > 0`. The
/// weight is fit offline by [`train`]; `weight = 0` is the no-guidance baseline.
#[derive(Clone, Copy, Debug)]
pub struct Oracle {
    pub weight: i64,
}

impl Oracle {
    pub fn baseline() -> Self {
        Oracle { weight: 0 }
    }
    /// The `#heuristic` level for an atom (0 ⇒ emit no hint).
    pub fn level(&self, f: &AtomFeatures) -> i64 {
        self.weight * f.degree as i64
    }
}

/// Offline fit: pick the candidate weight minimizing a measured cost (e.g. total
/// clingo choices over the training instances). This is the "training strictly
/// offline" of §5.4 in miniature — the caller supplies the cost oracle.
pub fn train<F: Fn(i64) -> u64>(candidates: &[i64], cost: F) -> Oracle {
    let weight = candidates
        .iter()
        .copied()
        .min_by_key(|&w| cost(w))
        .unwrap_or(0);
    Oracle { weight }
}

/// A small message-passing graph network over the constraint graph. Node feature
/// starts as normalized degree; each round replaces it with
/// `tanh(w_self·h_v + w_nbr·mean_{u∈N(v)} h_u)` (a GCN layer with mean
/// aggregation). After `rounds` rounds the score reflects the node's whole
/// `rounds`-hop neighbourhood — a node in a dense sub-structure (e.g. a clique
/// that forces a colouring conflict) scores higher than a peripheral one.
#[derive(Clone, Copy, Debug)]
pub struct Gnn {
    pub rounds: usize,
    pub w_self: f64,
    pub w_nbr: f64,
    /// Output scale; negative flips the ranking (the anti-oracle control).
    pub w_out: f64,
}

impl Gnn {
    /// Sensible offline-set weights: propagate neighbourhood constraint mass.
    pub fn trained() -> Self {
        Gnn {
            rounds: 2,
            w_self: 1.0,
            w_nbr: 1.0,
            w_out: 1.0,
        }
    }
    /// The negative control: same network, ranking inverted (least-constrained
    /// first). Used by the ablation to show it discriminates good order from bad.
    pub fn anti() -> Self {
        Gnn {
            w_out: -1.0,
            ..Gnn::trained()
        }
    }

    /// Per-node score from message passing. `adj[v]` are v's neighbour indices.
    pub fn scores(&self, adj: &[Vec<usize>]) -> Vec<f64> {
        let n = adj.len();
        let maxdeg = adj.iter().map(|a| a.len()).max().unwrap_or(1).max(1) as f64;
        // initial feature: normalized degree in [0, 1].
        let mut h: Vec<f64> = adj.iter().map(|a| a.len() as f64 / maxdeg).collect();
        for _ in 0..self.rounds {
            let mut hn = vec![0.0; n];
            for v in 0..n {
                let mean = if adj[v].is_empty() {
                    0.0
                } else {
                    adj[v].iter().map(|&u| h[u]).sum::<f64>() / adj[v].len() as f64
                };
                hn[v] = (self.w_self * h[v] + self.w_nbr * mean).tanh();
            }
            h = hn;
        }
        h.iter().map(|&x| self.w_out * x).collect()
    }

    /// Integer `#heuristic` levels: higher score ⇒ higher level (decided first).
    /// Scores are ranked so levels are dense small integers (robust to scale).
    pub fn levels(&self, adj: &[Vec<usize>]) -> Vec<i64> {
        let s = self.scores(adj);
        let mut idx: Vec<usize> = (0..s.len()).collect();
        idx.sort_by(|&a, &b| s[a].partial_cmp(&s[b]).unwrap());
        let mut level = vec![0i64; s.len()];
        for (rank, &node) in idx.iter().enumerate() {
            level[node] = rank as i64 + 1; // 1..=n, highest score → highest level
        }
        level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_emits_no_levels() {
        let o = Oracle::baseline();
        assert_eq!(o.level(&AtomFeatures { degree: 9 }), 0);
    }

    #[test]
    fn higher_degree_gets_higher_level() {
        let o = Oracle { weight: 4 };
        assert!(o.level(&AtomFeatures { degree: 5 }) > o.level(&AtomFeatures { degree: 2 }));
    }

    #[test]
    fn train_picks_min_cost() {
        // cost minimized at weight 3.
        let o = train(&[0, 1, 3, 5], |w| ((w - 3) * (w - 3)) as u64);
        assert_eq!(o.weight, 3);
    }

    #[test]
    fn gnn_ranks_central_node_above_leaf() {
        // star + a triangle: node 0 central (triangle 0-1-2), node 3 a leaf off 0.
        // adjacency (undirected):
        let adj = vec![
            vec![1, 2, 3], // 0: in the triangle + leaf
            vec![0, 2],    // 1: triangle
            vec![0, 1],    // 2: triangle
            vec![0],       // 3: leaf
        ];
        let g = Gnn::trained();
        let s = g.scores(&adj);
        assert!(s[0] > s[3], "central node must outscore the leaf");
        assert!(s[1] > s[3], "triangle node must outscore the leaf");
        // levels are a dense ranking; the leaf gets the lowest level.
        let lv = g.levels(&adj);
        assert_eq!(*lv.iter().min().unwrap(), 1);
        assert!(lv[0] > lv[3]);
    }

    #[test]
    fn anti_oracle_inverts_ranking() {
        // symmetric triangle (0,1,2) + leaf 3 off node 0 ⇒ node 0 uniquely central.
        let adj = vec![vec![1, 2, 3], vec![0, 2], vec![0, 1], vec![0]];
        let good = Gnn::trained().levels(&adj);
        let anti = Gnn::anti().levels(&adj);
        // the leaf (node 3) is uniquely least-central: the good oracle ranks it
        // lowest, the anti-oracle ranks it highest — the inversion the control needs.
        assert_eq!(
            good[3],
            *good.iter().min().unwrap(),
            "good ranks leaf lowest"
        );
        assert_eq!(
            anti[3],
            *anti.iter().max().unwrap(),
            "anti ranks leaf highest"
        );
    }
}
