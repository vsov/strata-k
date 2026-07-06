//! Provenance circuits and weighted model counting. [Phase 4]
//!
//! A derived tuple's provenance compiles to a circuit over the probabilistic
//! input facts (the leaves): an `And` is a conjunction of independent
//! sub-provenances, an `Or` a disjunction of mutually exclusive ones — the
//! **decomposable / deterministic** shape (SDD / d-DNNF class) on which weighted
//! model counting is exact and linear:
//!
//!   WMC(And) = ∏ children,   WMC(Or) = Σ children,   WMC(Leaf i) = p[i].
//!
//! The circuit is compiled once and re-evaluated as the leaf probabilities
//! change (each training epoch), and it is **differentiable**: `grad` backprops
//! `∂WMC/∂p[i]` for every leaf, the interface a neural layer trains through.

/// A circuit node. Children indices are always smaller than the node's own
/// index (bottom-up construction), so a single forward/reverse pass suffices.
#[derive(Clone, Debug)]
pub enum Node {
    /// Probabilistic input fact `i` (its probability is `p[i]`).
    Leaf(usize),
    /// The *absence* of probabilistic fact `i` — the dual literal `x̄` of the
    /// spec's provenance DAG ({AND, OR, NEG-leaf, LEAF}); value `1 - p[i]`.
    NegLeaf(usize),
    /// Conjunction of independent sub-provenances (decomposable).
    And(Vec<usize>),
    /// Disjunction of mutually exclusive sub-provenances (deterministic).
    Or(Vec<usize>),
    /// The constant `1` (true) — an empty conjunction.
    True,
    /// The constant `0` (false) — an empty disjunction.
    False,
}

/// A provenance circuit rooted at `root`, over `num_leaves` probabilistic facts.
#[derive(Clone, Debug)]
pub struct Circuit {
    pub nodes: Vec<Node>,
    pub root: usize,
    pub num_leaves: usize,
}

impl Circuit {
    fn eval(&self, p: &[f64]) -> Vec<f64> {
        let mut val = vec![0.0f64; self.nodes.len()];
        for i in 0..self.nodes.len() {
            val[i] = match &self.nodes[i] {
                Node::Leaf(l) => p[*l],
                Node::NegLeaf(l) => 1.0 - p[*l],
                Node::And(cs) => cs.iter().map(|&c| val[c]).product(),
                Node::Or(cs) => cs.iter().map(|&c| val[c]).sum(),
                Node::True => 1.0,
                Node::False => 0.0,
            };
        }
        val
    }

    /// Weighted model count: the exact probability of the root, given each
    /// leaf's probability `p[i]`.
    pub fn wmc(&self, p: &[f64]) -> f64 {
        if self.nodes.is_empty() {
            return 0.0;
        }
        self.eval(p)[self.root]
    }

    /// Weighted model count and its gradient `∂WMC/∂p[i]` for every leaf.
    pub fn grad(&self, p: &[f64]) -> (f64, Vec<f64>) {
        if self.nodes.is_empty() {
            return (0.0, vec![0.0; self.num_leaves]);
        }
        let val = self.eval(p);
        let mut node_g = vec![0.0f64; self.nodes.len()];
        let mut leaf_g = vec![0.0f64; self.num_leaves];
        node_g[self.root] = 1.0;
        for i in (0..self.nodes.len()).rev() {
            let g = node_g[i];
            if g == 0.0 {
                continue;
            }
            match &self.nodes[i] {
                Node::Leaf(l) => leaf_g[*l] += g,
                Node::NegLeaf(l) => leaf_g[*l] -= g, // ∂(1-p)/∂p = -1
                Node::And(cs) => {
                    // ∂(∏ c)/∂c_j = ∏_{k≠j} c_k — computed excluding j to stay
                    // correct when some child is zero.
                    for (jpos, &cj) in cs.iter().enumerate() {
                        let sib: f64 = cs
                            .iter()
                            .enumerate()
                            .filter(|(k, _)| *k != jpos)
                            .map(|(_, &c)| val[c])
                            .product();
                        node_g[cj] += g * sib;
                    }
                }
                Node::Or(cs) => {
                    for &cj in cs {
                        node_g[cj] += g;
                    }
                }
                Node::True | Node::False => {}
            }
        }
        (val[self.root], leaf_g)
    }
}

/// Builder that hash-conses-free-ly appends nodes (children before parents).
#[derive(Default)]
pub struct Builder {
    nodes: Vec<Node>,
    num_leaves: usize,
}

impl Builder {
    pub fn new() -> Self {
        Builder::default()
    }
    pub fn leaf(&mut self, i: usize) -> usize {
        self.num_leaves = self.num_leaves.max(i + 1);
        self.push(Node::Leaf(i))
    }
    pub fn neg_leaf(&mut self, i: usize) -> usize {
        self.num_leaves = self.num_leaves.max(i + 1);
        self.push(Node::NegLeaf(i))
    }
    pub fn and(&mut self, cs: Vec<usize>) -> usize {
        self.push(Node::And(cs))
    }
    pub fn or(&mut self, cs: Vec<usize>) -> usize {
        self.push(Node::Or(cs))
    }
    pub fn tru(&mut self) -> usize {
        self.push(Node::True)
    }
    pub fn fals(&mut self) -> usize {
        self.push(Node::False)
    }
    fn push(&mut self, n: Node) -> usize {
        self.nodes.push(n);
        self.nodes.len() - 1
    }
    pub fn finish(self, root: usize) -> Circuit {
        Circuit {
            nodes: self.nodes,
            root,
            num_leaves: self.num_leaves,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wmc_of_and_or() {
        // (l0 ∧ l1) ∨ (l0 ∧ l2), mutually exclusive on l1/l2 disjuncts.
        let mut b = Builder::new();
        let l0 = b.leaf(0);
        let l1 = b.leaf(1);
        let l2 = b.leaf(2);
        let a1 = b.and(vec![l0, l1]);
        let a2 = b.and(vec![l0, l2]);
        let root = b.or(vec![a1, a2]);
        let c = b.finish(root);
        let p = [0.5, 0.3, 0.4];
        // 0.5*0.3 + 0.5*0.4 = 0.35
        assert!((c.wmc(&p) - 0.35).abs() < 1e-12);
    }

    #[test]
    fn gradient_matches_finite_difference() {
        let mut b = Builder::new();
        let l: Vec<usize> = (0..4).map(|i| b.leaf(i)).collect();
        let a1 = b.and(vec![l[0], l[1]]);
        let a2 = b.and(vec![l[2], l[3]]);
        let a3 = b.and(vec![l[0], l[3]]);
        let root = b.or(vec![a1, a2, a3]);
        let c = b.finish(root);

        let p = [0.2, 0.7, 0.5, 0.9];
        let (_, g) = c.grad(&p);
        let eps = 1e-6;
        for i in 0..4 {
            let mut pp = p;
            pp[i] += eps;
            let mut pm = p;
            pm[i] -= eps;
            let num = (c.wmc(&pp) - c.wmc(&pm)) / (2.0 * eps);
            assert!((g[i] - num).abs() < 1e-6, "leaf {i}: {} vs {num}", g[i]);
        }
    }

    #[test]
    fn handles_zero_child_in_and() {
        // Gradient through an And where one child is 0 must still be correct.
        let mut b = Builder::new();
        let l0 = b.leaf(0);
        let l1 = b.leaf(1);
        let root = b.and(vec![l0, l1]);
        let c = b.finish(root);
        let p = [0.0, 0.6];
        let (v, g) = c.grad(&p);
        assert_eq!(v, 0.0);
        assert!((g[0] - 0.6).abs() < 1e-12); // ∂/∂l0 = l1 = 0.6
        assert!((g[1] - 0.0).abs() < 1e-12); // ∂/∂l1 = l0 = 0.0
    }
}
