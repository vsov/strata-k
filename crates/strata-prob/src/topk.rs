//! Top-k proofs. [Phase 4]
//!
//! Exact WMC sums over *all* proofs; that is #P-hard in general, so the soft
//! pipeline keeps only the `k` highest-probability proofs of a tuple (the
//! diff-top-k of Scallop). The kept proofs are a lower bound on the marginal and
//! a sparse, differentiable surrogate — cheap and, for peaked distributions,
//! close to exact.

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

/// Top-k weighted model count: the summed probability of the kept (mutually
/// exclusive) proofs — a lower bound on the exact marginal that equals it once
/// `k` covers every proof.
pub fn topk_wmc(proofs: &[Vec<usize>], p: &[f64], k: usize) -> f64 {
    top_k(proofs, p, k).iter().map(|(s, _)| s).sum()
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
}
