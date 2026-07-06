//! Top-k proofs. [Phase 4 / Prov_k]
//!
//! Exact WMC sums over *all* proofs; that is #P-hard in general, so the soft
//! pipeline keeps only the `k` highest-probability proofs of a tuple (the
//! diff-top-k of Scallop). The kept proofs are a lower bound on the marginal and
//! a sparse, differentiable surrogate — cheap and, for peaked distributions,
//! close to exact.
//!
//! Two APIs: the unsigned [`top_k`]/[`topk_wmc`] pair scores positive-leaf
//! proofs and *sums* them — exact only when the proofs are mutually exclusive
//! events (categorical leaves, as in `mnist_sum`). The signed
//! [`top_k_signed`]/[`topk_circuit`] pair is the `Prov_k` engine over
//! independent Bernoulli leaves: selection by proof weight, then **exact WMC of
//! the union** of the kept proofs via [`compile_exact`](crate::compile) — a
//! guaranteed lower bound even when proofs overlap (a plain sum is not: it can
//! exceed the true marginal).

use crate::circuit::Circuit;
use crate::compile::compile_exact;

/// The `k` highest-probability proofs, each scored by the product of its leaf
/// probabilities, sorted descending.
pub fn top_k(proofs: &[Vec<usize>], p: &[f64], k: usize) -> Vec<(f64, Vec<usize>)> {
    let mut scored: Vec<(f64, Vec<usize>)> = proofs
        .iter()
        .map(|pr| (pr.iter().map(|&l| p[l]).product::<f64>(), pr.clone()))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);
    scored
}

/// Top-k weighted model count: the summed probability of the kept proofs.
/// Valid **only for mutually exclusive** proofs (categorical leaves) — then it
/// is a lower bound that equals the exact marginal once `k` covers every proof.
/// For overlapping proofs over independent Bernoulli leaves use
/// [`topk_circuit`], whose union-WMC is a lower bound unconditionally.
pub fn topk_wmc(proofs: &[Vec<usize>], p: &[f64], k: usize) -> f64 {
    top_k(proofs, p, k).iter().map(|(s, _)| s).sum()
}

/// The weight of one signed proof over independent Bernoulli leaves:
/// `∏ p[l-1]` for positive literals, `∏ (1 - p[-l-1])` for dual literals.
fn signed_weight(proof: &[i64], p: &[f64]) -> f64 {
    proof
        .iter()
        .map(|&l| {
            let i = (l.abs() - 1) as usize;
            if l > 0 {
                p[i]
            } else {
                1.0 - p[i]
            }
        })
        .product()
}

/// The `k` best proofs by signed weight (ties broken lexicographically, so the
/// selection is deterministic and merge-order-invariant). Proofs come back
/// sorted by the same (weight desc, literals asc) order.
pub fn top_k_signed(proofs: &[Vec<i64>], p: &[f64], k: usize) -> Vec<Vec<i64>> {
    let mut scored: Vec<(f64, Vec<i64>)> = proofs
        .iter()
        .map(|pr| {
            let mut s = pr.clone();
            s.sort_unstable();
            (signed_weight(&s, p), s)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    scored.truncate(k);
    scored.into_iter().map(|(_, pr)| pr).collect()
}

/// The `Prov_k` circuit: exact WMC/grad over the **union** of the top-k proofs.
/// A guaranteed lower bound on the exact marginal (the kept proofs are a subset
/// of all proofs), monotone in `k`, equal to exact once `k` covers every proof.
pub fn topk_circuit(proofs: &[Vec<i64>], p: &[f64], k: usize, num_leaves: usize) -> Circuit {
    compile_exact(&top_k_signed(proofs, p, k), num_leaves)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_highest() {
        // three proofs with products 0.5, 0.06, 0.2
        let proofs = vec![vec![0], vec![1, 2], vec![3]];
        let p = [0.5, 0.3, 0.2, 0.2];
        let top = top_k(&proofs, &p, 2);
        assert_eq!(top.len(), 2);
        assert!((top[0].0 - 0.5).abs() < 1e-12);
        assert!((top[1].0 - 0.2).abs() < 1e-12);
        assert_eq!(top[0].1, vec![0]);
    }

    #[test]
    fn full_k_equals_total() {
        let proofs = vec![vec![0], vec![1], vec![2]];
        let p = [0.2, 0.3, 0.4];
        // mutually exclusive → total 0.9; top-3 sums all, top-1 only the best.
        assert!((topk_wmc(&proofs, &p, 3) - 0.9).abs() < 1e-12);
        assert!((topk_wmc(&proofs, &p, 1) - 0.4).abs() < 1e-12);
        assert!(topk_wmc(&proofs, &p, 1) < topk_wmc(&proofs, &p, 3));
    }

    #[test]
    fn signed_union_is_a_lower_bound_where_a_sum_is_not() {
        // Overlapping proofs {l0}, {l1} at p = 0.9/0.9: sum = 1.8 (not even a
        // probability); union-WMC = 1 - 0.01 = 0.99 — the exact marginal.
        let proofs = vec![vec![1], vec![2]];
        let p = [0.9, 0.9];
        let full = topk_circuit(&proofs, &p, 2, 2).wmc(&p);
        assert!((full - 0.99).abs() < 1e-12);
        let k1 = topk_circuit(&proofs, &p, 1, 2).wmc(&p);
        assert!((k1 - 0.9).abs() < 1e-12);
        assert!(k1 <= full);
    }

    #[test]
    fn monotone_in_k_and_converges_to_exact() {
        // Chain-and-shortcut proofs with shared leaves.
        let proofs = vec![vec![1, 2], vec![1, 3], vec![2, 3], vec![4]];
        let p = [0.6, 0.5, 0.4, 0.3];
        let exact = crate::compile::compile_exact(&proofs, 4).wmc(&p);
        let mut prev = 0.0;
        for k in 1..=4 {
            let lb = topk_circuit(&proofs, &p, k, 4).wmc(&p);
            assert!(lb + 1e-12 >= prev, "k={k}: {lb} < {prev}");
            assert!(lb <= exact + 1e-12, "k={k}: {lb} > exact {exact}");
            prev = lb;
        }
        assert!((prev - exact).abs() < 1e-12, "k=all must equal exact");
    }

    #[test]
    fn selection_is_deterministic_under_permutation() {
        // Equal-weight proofs: the lexicographic tie-break makes the kept set
        // independent of input order (the spec's merge-order invariance).
        let a = vec![vec![1, 2], vec![3, 4], vec![5, 6]];
        let mut b = a.clone();
        b.reverse();
        let p = [0.5; 6];
        assert_eq!(top_k_signed(&a, &p, 2), top_k_signed(&b, &p, 2));
    }

    #[test]
    fn dual_literals_in_selection_weight() {
        // {+l0, -l1} weighs 0.8·(1-0.3)=0.56 > {+l1} at 0.3.
        let proofs = vec![vec![1, -2], vec![2]];
        let p = [0.8, 0.3];
        let kept = top_k_signed(&proofs, &p, 1);
        assert_eq!(kept, vec![vec![-2, 1]]);
    }
}
